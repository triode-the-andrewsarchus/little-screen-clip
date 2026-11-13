//! The Rough.js 4.6.4 port: the seeded generators that turn element outlines
//! into sketched `Drawing`s — rectangle, polygon, linear, rounded, curve and
//! ellipse strokes with the seven fillers Excalidraw passes through unvalidated
//! (solid, hachure, cross-hatch, zigzag, zigzag-line, dots and dashed) — and
//! the PRNG they draw from, reproduced operation for operation. The `Point`,
//! `PathOperation` and `Path` primitives it emits are `geometry`'s.

use std::collections::TryReserveError;

use super::geometry::{ecmascript_round, Path, PathOperation, Point};
use crate::portable_math::{atan, cos, hypot, sin, sin_cos, square};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GenerationLimit {
    GeometrySegments,
    WorkUnits,
}

#[derive(Debug)]
pub(super) enum GenerationError {
    Coordinate {
        value: f64,
        geometry_segments: u64,
    },
    Limit {
        kind: GenerationLimit,
        attempted: u64,
        geometry_segments: u64,
    },
    Memory,
}

impl From<TryReserveError> for GenerationError {
    fn from(_error: TryReserveError) -> Self {
        Self::Memory
    }
}

pub(super) struct GenerationBudget {
    coordinate_limit: f64,
    operation_limit: u64,
    operation_limit_kind: GenerationLimit,
    base_work_units: u64,
    operations: u64,
    work_limit: u64,
    work: u64,
    discard_next_move: bool,
}

impl GenerationBudget {
    pub(super) const fn new(
        geometry_limit: u64,
        element_work_limit: u64,
        base_work_units: u64,
    ) -> Self {
        // Generate the first crossing command so aggregate-fatal precedence can
        // be decided before element-local recovery.
        let geometry_generation_limit = geometry_limit.saturating_add(1);
        let work_operation_limit = element_work_limit
            .saturating_sub(base_work_units)
            .saturating_div(32)
            .saturating_add(1);
        let (operation_limit, operation_limit_kind) =
            if geometry_generation_limit <= work_operation_limit {
                (geometry_generation_limit, GenerationLimit::GeometrySegments)
            } else {
                (work_operation_limit, GenerationLimit::WorkUnits)
            };
        Self {
            coordinate_limit: f64::MAX,
            operation_limit,
            operation_limit_kind,
            base_work_units,
            operations: 0,
            work_limit: element_work_limit,
            work: 0,
            discard_next_move: false,
        }
    }

    /// The guard work spent so far, apart from the base work and the
    /// operations it bounds.
    pub(super) const fn spent_work(&self) -> u64 {
        self.work
    }

    pub(super) fn work(&mut self, amount: u64) -> Result<(), GenerationError> {
        let work = self.work.saturating_add(amount);
        let attempted = self.base_work_units.saturating_add(work);
        if attempted > self.work_limit {
            return Err(GenerationError::Limit {
                kind: GenerationLimit::WorkUnits,
                attempted,
                geometry_segments: self.operations,
            });
        }
        self.work = work;
        Ok(())
    }

    pub(super) fn with_coordinate_limit(mut self, limit: f64) -> Self {
        self.coordinate_limit = limit;
        self
    }

    pub(super) fn coordinate(&self, value: f64) -> Result<(), GenerationError> {
        super::coordinates::CoordinateRange(self.coordinate_limit)
            .value(value)
            .map_err(|value| GenerationError::Coordinate {
                value,
                geometry_segments: self.operations,
            })
    }

    pub(super) fn point(&self, point: Point) -> Result<(), GenerationError> {
        self.coordinate(point.x)?;
        self.coordinate(point.y)
    }

    fn points(&self, points: &[Point]) -> Result<(), GenerationError> {
        for point in points {
            self.point(*point)?;
        }
        Ok(())
    }

    pub(super) fn operation(&mut self) -> Result<(), GenerationError> {
        let attempted = self.operations.saturating_add(1);
        if attempted > self.operation_limit {
            return Err(GenerationError::Limit {
                kind: self.operation_limit_kind,
                attempted: if self.operation_limit_kind == GenerationLimit::WorkUnits {
                    self.base_work_units
                        .saturating_add(attempted.saturating_mul(32))
                } else {
                    self.operation_limit
                },
                geometry_segments: self.operation_limit,
            });
        }
        self.operations = attempted;
        Ok(())
    }

    fn push_operation(
        &mut self,
        operations: &mut Vec<PathOperation>,
        operation: PathOperation,
    ) -> Result<(), GenerationError> {
        super::coordinates::CoordinateRange(self.coordinate_limit)
            .operation(&operation, super::coordinates::IDENTITY)
            .map_err(|value| GenerationError::Coordinate {
                value,
                geometry_segments: self.operations,
            })?;
        if self.discard_next_move && matches!(operation, PathOperation::Move(_)) {
            self.discard_next_move = false;
            self.work(1)?;
            return Ok(());
        }
        self.operation()?;
        operations.try_reserve(1)?;
        operations.push(operation);
        Ok(())
    }

    fn discard_next_move(&mut self) {
        self.discard_next_move = true;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FillKind {
    Solid,
    Sketch,
}

#[derive(Debug, PartialEq)]
pub(super) struct Fill {
    pub(super) path: Path,
    pub(super) kind: FillKind,
}

#[derive(Debug, PartialEq)]
pub(super) struct Drawing {
    pub(super) outline: Path,
    pub(super) fill: Option<Fill>,
    pub(super) shape: DrawableShape,
    pub(super) decorations: Vec<Decoration>,
}

#[derive(Debug, PartialEq)]
pub(super) struct Decoration {
    pub(super) drawing: Drawing,
    pub(super) fill_paint: DecorationFillPaint,
    pub(super) dash: DecorationDash,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DecorationFillPaint {
    Stroke,
    Background,
}

/// The `strokeLineDash` an arrowhead is drawn with: the state of the options
/// object the reference's `getArrowheadShapes` shares between the outline and
/// both heads at the moment the head is generated (#171). A closed head
/// deletes the pattern; an open head other than `crowfoot_one` replaces a
/// dotted element's pattern with the shortened one and deletes any other; a
/// `crowfoot_one` draws whatever pattern the options carry when it is reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DecorationDash {
    /// No pattern: a solid head.
    Solid,
    /// The element's own dashed pattern, `[8, 8 + strokeWidth]`.
    Dashed,
    /// The element's own dotted pattern, `[1.5, 6 + strokeWidth]`.
    Dotted,
    /// The open heads' dotted pattern, `getDashArrayDotted(strokeWidth - 1)`
    /// with its gap shortened by one: `[1.5, 4 + strokeWidth]`.
    DottedHead,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DrawableShape {
    Curve,
    Ellipse,
    Path,
    Polygon,
    Rectangle,
}

impl DrawableShape {
    pub(super) const fn svg_uses_even_odd_fill(self) -> bool {
        matches!(self, Self::Curve | Self::Polygon)
    }

    #[cfg(any(feature = "png", feature = "pdf"))]
    pub(super) const fn canvas_uses_even_odd_fill(self) -> bool {
        matches!(self, Self::Curve | Self::Path | Self::Polygon)
    }
}

impl Drawing {
    pub(super) fn operation_count(&self) -> usize {
        self.outline.operations.len()
            + self
                .fill
                .as_ref()
                .map_or(0, |fill| fill.path.operations.len())
            + self
                .decorations
                .iter()
                .map(|decoration| decoration.drawing.operation_count())
                .sum::<usize>()
    }
}

/// The `fillStyle` values Rough.js 4.6.4's `getFiller` dispatches, which the
/// reference passes through from the element unvalidated (#137).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FillStyle {
    Solid,
    Hachure,
    CrossHatch,
    Zigzag,
    ZigzagLine,
    Dots,
    Dashed,
}

impl FillStyle {
    /// The filler for a normalized `fillStyle`. Normalization admits exactly
    /// these seven spellings; `solid` is also what an unsupported value
    /// substitutes.
    pub(super) fn from_normalized(fill_style: &str) -> Self {
        match fill_style {
            "hachure" => Self::Hachure,
            "cross-hatch" => Self::CrossHatch,
            "zigzag" => Self::Zigzag,
            "zigzag-line" => Self::ZigzagLine,
            "dots" => Self::Dots,
            "dashed" => Self::Dashed,
            _ => Self::Solid,
        }
    }
}

/// What a pattern filler reads of Excalidraw's `generateRoughOptions` besides
/// the polygon it fills: the element's own stroke width (`hachureGap` is four
/// times it, `fillWeight` half of it, whatever the stroke style adds to the
/// outline's width), the adjusted roughness, `preserveVertices`, and, for the
/// dot filler's ellipses alone, `disableMultiStroke` and `curveFitting` (1 for
/// an ellipse element, Rough.js's 0.95 otherwise) together with the element
/// seed that scatters the dots.
#[derive(Clone, Copy)]
struct FillOptions {
    style: FillStyle,
    source_stroke_width: f64,
    roughness: f64,
    preserve_vertices: bool,
    disable_multi_stroke: bool,
    curve_fitting: f64,
    seed: i32,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn rectangle_drawing(
    width: f64,
    height: f64,
    seed: i32,
    roughness: f64,
    preserve_vertices: bool,
    disable_multi_stroke: bool,
    fill_style: Option<FillStyle>,
    source_stroke_width: f64,
    budget: &mut GenerationBudget,
) -> Result<Drawing, GenerationError> {
    let top_left = Point { x: 0.0, y: 0.0 };
    let top_right = Point { x: width, y: 0.0 };
    let bottom_right = Point {
        x: width,
        y: height,
    };
    let bottom_left = Point { x: 0.0, y: height };
    let segments = [
        (top_left, top_right),
        (top_right, bottom_right),
        (bottom_right, bottom_left),
        (bottom_left, top_left),
    ];
    let mut random = Random::new(seed);
    let mut operations = Vec::new();
    for (start, end) in segments {
        double_line_mode(
            start,
            end,
            roughness,
            preserve_vertices,
            disable_multi_stroke,
            false,
            &mut random,
            &mut operations,
            budget,
        )?;
    }
    let fill = fill_style
        .map(|style| {
            fill_polygon(
                &[top_left, top_right, bottom_right, bottom_left],
                FillOptions {
                    style,
                    source_stroke_width,
                    roughness,
                    preserve_vertices,
                    disable_multi_stroke,
                    curve_fitting: 0.95,
                    seed,
                },
                &mut random,
                budget,
            )
        })
        .transpose()?;
    Ok(Drawing {
        outline: Path { operations },
        fill,
        shape: DrawableShape::Rectangle,
        decorations: Vec::new(),
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn polygon_drawing(
    points: &[Point],
    seed: i32,
    roughness: f64,
    preserve_vertices: bool,
    disable_multi_stroke: bool,
    fill_style: Option<FillStyle>,
    source_stroke_width: f64,
    budget: &mut GenerationBudget,
) -> Result<Drawing, GenerationError> {
    let mut operations = Vec::new();
    let mut random = Random::new(seed);
    for pair in points.windows(2) {
        double_line_mode(
            pair[0],
            pair[1],
            roughness,
            preserve_vertices,
            disable_multi_stroke,
            false,
            &mut random,
            &mut operations,
            budget,
        )?;
    }
    if points.len() > 2 {
        double_line_mode(
            *points.last().expect("a polygon has points"),
            points[0],
            roughness,
            preserve_vertices,
            disable_multi_stroke,
            false,
            &mut random,
            &mut operations,
            budget,
        )?;
    }
    let fill = fill_style
        .map(|style| {
            fill_polygon(
                points,
                FillOptions {
                    style,
                    source_stroke_width,
                    roughness,
                    preserve_vertices,
                    disable_multi_stroke,
                    curve_fitting: 0.95,
                    seed,
                },
                &mut random,
                budget,
            )
        })
        .transpose()?;
    Ok(Drawing {
        outline: Path { operations },
        fill,
        shape: DrawableShape::Polygon,
        decorations: Vec::new(),
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn linear_drawing(
    points: &[Point],
    closed: bool,
    seed: i32,
    roughness: f64,
    preserve_vertices: bool,
    disable_multi_stroke: bool,
    fill_style: Option<FillStyle>,
    source_stroke_width: f64,
    budget: &mut GenerationBudget,
) -> Result<Drawing, GenerationError> {
    let mut operations = Vec::new();
    let mut random = Random::new(seed);
    for pair in points.windows(2) {
        double_line_mode(
            pair[0],
            pair[1],
            roughness,
            preserve_vertices,
            disable_multi_stroke,
            false,
            &mut random,
            &mut operations,
            budget,
        )?;
    }
    let fill = if closed {
        fill_style
            .map(|style| {
                fill_polygon(
                    points,
                    FillOptions {
                        style,
                        source_stroke_width,
                        roughness,
                        preserve_vertices,
                        disable_multi_stroke,
                        curve_fitting: 0.95,
                        seed,
                    },
                    &mut random,
                    budget,
                )
            })
            .transpose()?
    } else {
        None
    };
    Ok(Drawing {
        outline: Path { operations },
        fill,
        shape: DrawableShape::Polygon,
        decorations: Vec::new(),
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn rounded_drawing(
    start: Point,
    segments: &[RoundedSegment],
    fill_points: &[Point],
    seed: i32,
    roughness: f64,
    preserve_vertices: bool,
    disable_multi_stroke: bool,
    fill_style: Option<FillStyle>,
    source_stroke_width: f64,
    budget: &mut GenerationBudget,
) -> Result<Drawing, GenerationError> {
    let mut operations = Vec::new();
    let mut random = Random::new(seed);
    rounded_operations(
        start,
        segments,
        roughness,
        preserve_vertices,
        disable_multi_stroke,
        &mut random,
        &mut operations,
        budget,
        false,
    )?;
    let fill = match fill_style {
        Some(FillStyle::Solid) => {
            let mut fill_operations = Vec::new();
            rounded_operations(
                start,
                segments,
                if roughness == 0.0 {
                    0.0
                } else {
                    roughness + 0.8
                },
                preserve_vertices,
                true,
                &mut random,
                &mut fill_operations,
                budget,
                true,
            )?;
            let mut first = true;
            fill_operations.retain(|operation| {
                if matches!(operation, PathOperation::Move(_)) {
                    let retain = first;
                    first = false;
                    retain
                } else {
                    true
                }
            });
            Some(Fill {
                path: Path {
                    operations: fill_operations,
                },
                kind: FillKind::Solid,
            })
        }
        Some(style) => Some(fill_polygon(
            fill_points,
            FillOptions {
                style,
                source_stroke_width,
                roughness,
                preserve_vertices,
                disable_multi_stroke,
                curve_fitting: 0.95,
                seed,
            },
            &mut random,
            budget,
        )?),
        None => None,
    };
    Ok(Drawing {
        outline: Path { operations },
        fill,
        shape: DrawableShape::Path,
        decorations: Vec::new(),
    })
}

#[allow(clippy::too_many_arguments)]
fn rounded_operations(
    start: Point,
    segments: &[RoundedSegment],
    roughness: f64,
    preserve_vertices: bool,
    disable_multi_stroke: bool,
    random: &mut Random,
    operations: &mut Vec<PathOperation>,
    budget: &mut GenerationBudget,
    discard_repeated_moves: bool,
) -> Result<(), GenerationError> {
    let mut current = start;
    let mut first_segment = true;
    for segment in segments {
        if discard_repeated_moves && !first_segment {
            budget.discard_next_move();
        }
        match *segment {
            RoundedSegment::Line(end) => {
                double_line_mode(
                    current,
                    end,
                    roughness,
                    preserve_vertices,
                    disable_multi_stroke,
                    false,
                    random,
                    operations,
                    budget,
                )?;
                current = end;
            }
            RoundedSegment::Quadratic { control, end } => {
                let control_1 = Point {
                    x: current.x + (control.x - current.x) * (2.0 / 3.0),
                    y: current.y + (control.y - current.y) * (2.0 / 3.0),
                };
                let control_2 = Point {
                    x: end.x + (control.x - end.x) * (2.0 / 3.0),
                    y: end.y + (control.y - end.y) * (2.0 / 3.0),
                };
                double_bezier_mode(
                    current,
                    control_1,
                    control_2,
                    end,
                    roughness,
                    preserve_vertices,
                    disable_multi_stroke,
                    random,
                    operations,
                    budget,
                )?;
                current = end;
            }
            RoundedSegment::Cubic {
                control_1,
                control_2,
                end,
            } => {
                double_bezier_mode(
                    current,
                    control_1,
                    control_2,
                    end,
                    roughness,
                    preserve_vertices,
                    disable_multi_stroke,
                    random,
                    operations,
                    budget,
                )?;
                current = end;
            }
        }
        first_segment = false;
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(super) enum RoundedSegment {
    Line(Point),
    Quadratic {
        control: Point,
        end: Point,
    },
    Cubic {
        control_1: Point,
        control_2: Point,
        end: Point,
    },
}

/// A curve's outline never reads `preserve_vertices`; its pattern fill does,
/// through `_line`, which jitters each fill line's endpoints unless the
/// vertices are preserved. The reference drives a rounded line or arrow with
/// `generateRoughOptions(element)` and no `continuousPath`, so the value is
/// the element's raw `roughness < 2` (#158), which the caller passes in.
#[allow(clippy::too_many_arguments)]
pub(super) fn curve_drawing(
    points: &[Point],
    closed: bool,
    seed: i32,
    roughness: f64,
    preserve_vertices: bool,
    disable_multi_stroke: bool,
    fill_style: Option<FillStyle>,
    source_stroke_width: f64,
    budget: &mut GenerationBudget,
) -> Result<Drawing, GenerationError> {
    let mut operations = Vec::new();
    let mut random = Random::new(seed);
    curve_stroke(
        points,
        1.0 * (1.0 + roughness * 0.2),
        roughness,
        &mut random,
        &mut operations,
        budget,
    )?;
    if !disable_multi_stroke {
        let mut overlay_random = Random::new(seed.wrapping_add(1));
        curve_stroke(
            points,
            1.5 * (1.0 + roughness * 0.22),
            roughness,
            &mut overlay_random,
            &mut operations,
            budget,
        )?;
    }
    let fill = if closed {
        fill_style
            .map(|style| {
                curve_fill_with_random(
                    points,
                    FillOptions {
                        style,
                        source_stroke_width,
                        roughness,
                        preserve_vertices,
                        disable_multi_stroke,
                        curve_fitting: 0.95,
                        seed,
                    },
                    &mut random,
                    budget,
                )
            })
            .transpose()?
    } else {
        None
    };
    Ok(Drawing {
        outline: Path { operations },
        fill,
        shape: DrawableShape::Curve,
        decorations: Vec::new(),
    })
}

/// The Rough.js background fill of a freedraw loop, which the reference draws
/// as a `generator.curve` with `stroke: "none"` over the simplified points.
/// `preserve_vertices` is the element's raw `roughness < 2`, as for
/// [`curve_drawing`] (#158).
///
/// `generator.curve` generates the outline before the fill and `stroke:
/// "none"` only drops it from the drawable afterwards, so the options'
/// seeded stream, which the outline's first pass and the fill share, reaches
/// the fill after the outline's `2 · (n + 2)` offsets; the multi-stroke
/// overlay draws from its own stream at `seed + 1` and moves this one by
/// nothing, whether or not `disable_multi_stroke` skips it. The outline's
/// draws are made and discarded here as the reference makes and discards
/// them (#170); nothing of the discarded outline is charged to the budget,
/// since no geometry of it is emitted.
#[allow(clippy::too_many_arguments)]
pub(super) fn curve_fill(
    points: &[Point],
    seed: i32,
    roughness: f64,
    preserve_vertices: bool,
    disable_multi_stroke: bool,
    fill_style: FillStyle,
    source_stroke_width: f64,
    budget: &mut GenerationBudget,
) -> Result<Fill, GenerationError> {
    let mut random = Random::new(seed);
    discard_curve_stroke(
        points,
        1.0 * (1.0 + roughness * 0.2),
        roughness,
        &mut random,
    );
    curve_fill_with_random(
        points,
        FillOptions {
            style: fill_style,
            source_stroke_width,
            roughness,
            preserve_vertices,
            disable_multi_stroke,
            curve_fitting: 0.95,
            seed,
        },
        &mut random,
        budget,
    )
}

fn curve_fill_with_random(
    points: &[Point],
    options: FillOptions,
    random: &mut Random,
    budget: &mut GenerationBudget,
) -> Result<Fill, GenerationError> {
    match options.style {
        FillStyle::Solid => {
            let fill_roughness = if options.roughness == 0.0 {
                0.0
            } else {
                options.roughness + 0.8
            };
            let mut operations = Vec::new();
            curve_stroke(
                points,
                1.0 * (1.0 + fill_roughness * 0.2),
                fill_roughness,
                random,
                &mut operations,
                budget,
            )?;
            Ok(Fill {
                path: Path { operations },
                kind: FillKind::Solid,
            })
        }
        _ => {
            let bezier = curve_to_bezier(points, budget)?;
            let fill_points = points_on_bezier_curves(
                &bezier,
                10.0,
                Some((1.0 + options.roughness) / 2.0),
                budget,
            )?;
            fill_polygon(&fill_points, options, random, budget)
        }
    }
}

pub(super) fn rounded_fill_points(
    start: Point,
    segments: &[RoundedSegment],
    roughness: f64,
    budget: &mut GenerationBudget,
) -> Result<Vec<Point>, GenerationError> {
    let mut current_points = Vec::new();
    budget.work(1)?;
    current_points.try_reserve(1)?;
    current_points.push(start);
    let mut pending_curve = Vec::new();

    for segment in segments {
        match *segment {
            RoundedSegment::Line(end) => {
                append_pending_curve(&mut current_points, &mut pending_curve, 1.0, budget)?;
                budget.work(1)?;
                current_points.try_reserve(1)?;
                current_points.push(end);
            }
            RoundedSegment::Quadratic { control, end } => {
                let current = pending_curve
                    .last()
                    .copied()
                    .or_else(|| current_points.last().copied())
                    .unwrap_or(start);
                let control_1 = Point {
                    x: current.x + 2.0 * (control.x - current.x) / 3.0,
                    y: current.y + 2.0 * (control.y - current.y) / 3.0,
                };
                let control_2 = Point {
                    x: end.x + 2.0 * (control.x - end.x) / 3.0,
                    y: end.y + 2.0 * (control.y - end.y) / 3.0,
                };
                if pending_curve.is_empty() {
                    budget.work(1)?;
                    pending_curve.try_reserve(1)?;
                    pending_curve.push(current);
                }
                budget.work(3)?;
                pending_curve.try_reserve(3)?;
                pending_curve.extend([control_1, control_2, end]);
            }
            RoundedSegment::Cubic {
                control_1,
                control_2,
                end,
            } => {
                if pending_curve.is_empty() {
                    budget.work(1)?;
                    pending_curve.try_reserve(1)?;
                    pending_curve.push(*current_points.last().unwrap_or(&start));
                }
                budget.work(3)?;
                pending_curve.try_reserve(3)?;
                pending_curve.extend([control_1, control_2, end]);
            }
        }
    }
    append_pending_curve(&mut current_points, &mut pending_curve, 1.0, budget)?;
    simplify(&current_points, (1.0 + roughness) / 2.0, budget)
}

fn append_pending_curve(
    points: &mut Vec<Point>,
    pending: &mut Vec<Point>,
    tolerance: f64,
    budget: &mut GenerationBudget,
) -> Result<(), GenerationError> {
    if pending.len() >= 4 {
        let sampled = points_on_bezier_curves(pending, tolerance, None, budget)?;
        budget.work(u64::try_from(sampled.len()).unwrap_or(u64::MAX))?;
        points.try_reserve(sampled.len())?;
        points.extend(sampled);
    }
    pending.clear();
    Ok(())
}

fn curve_to_bezier(
    points: &[Point],
    budget: &mut GenerationBudget,
) -> Result<Vec<Point>, GenerationError> {
    let mut output = Vec::new();
    if points.len() == 3 {
        budget.work(4)?;
        output.try_reserve_exact(4)?;
        output.extend([points[0], points[1], points[2], points[2]]);
        return Ok(output);
    }

    let mut expanded = Vec::new();
    budget.work(
        u64::try_from(points.len())
            .unwrap_or(u64::MAX)
            .saturating_add(2),
    )?;
    expanded.try_reserve_exact(points.len().saturating_add(2))?;
    expanded.push(points[0]);
    expanded.push(points[0]);
    for (index, point) in points.iter().copied().enumerate().skip(1) {
        expanded.push(point);
        if index == points.len() - 1 {
            expanded.push(point);
        }
    }
    let output_len = 1 + expanded.len().saturating_sub(3).saturating_mul(3);
    budget.work(u64::try_from(output_len).unwrap_or(u64::MAX))?;
    output.try_reserve_exact(output_len)?;
    output.push(expanded[0]);
    for index in 1..expanded.len() - 2 {
        let current = expanded[index];
        let next = expanded[index + 1];
        output.extend([
            Point {
                x: current.x + (next.x - expanded[index - 1].x) / 6.0,
                y: current.y + (next.y - expanded[index - 1].y) / 6.0,
            },
            Point {
                x: next.x + (current.x - expanded[index + 2].x) / 6.0,
                y: next.y + (current.y - expanded[index + 2].y) / 6.0,
            },
            next,
        ]);
    }
    Ok(output)
}

fn points_on_bezier_curves(
    points: &[Point],
    tolerance: f64,
    distance: Option<f64>,
    budget: &mut GenerationBudget,
) -> Result<Vec<Point>, GenerationError> {
    let mut sampled = Vec::new();
    for offset in (0..points.len().saturating_sub(1)).step_by(3) {
        if offset + 3 < points.len() {
            sample_bezier(&points[offset..offset + 4], tolerance, &mut sampled, budget)?;
        }
    }
    match distance {
        Some(distance) if distance > 0.0 => simplify(&sampled, distance, budget),
        _ => Ok(sampled),
    }
}

fn sample_bezier(
    points: &[Point],
    tolerance: f64,
    output: &mut Vec<Point>,
    budget: &mut GenerationBudget,
) -> Result<(), GenerationError> {
    budget.work(1)?;
    let mut ux = 3.0 * points[1].x - 2.0 * points[0].x - points[3].x;
    ux *= ux;
    let mut uy = 3.0 * points[1].y - 2.0 * points[0].y - points[3].y;
    uy *= uy;
    let mut vx = 3.0 * points[2].x - 2.0 * points[3].x - points[0].x;
    vx *= vx;
    let mut vy = 3.0 * points[2].y - 2.0 * points[3].y - points[0].y;
    vy *= vy;
    ux = ux.max(vx);
    uy = uy.max(vy);

    if ux + uy < tolerance {
        budget.work(2)?;
        output.try_reserve(2)?;
        if output.is_empty()
            || output
                .last()
                .is_some_and(|last| point_distance(*last, points[0]) > 1.0)
        {
            output.push(points[0]);
        }
        output.push(points[3]);
        return Ok(());
    }

    let q1 = midpoint(points[0], points[1]);
    let q2 = midpoint(points[1], points[2]);
    let q3 = midpoint(points[2], points[3]);
    let r1 = midpoint(q1, q2);
    let r2 = midpoint(q2, q3);
    let center = midpoint(r1, r2);
    if center == points[0] || center == points[3] {
        return Err(GenerationError::Limit {
            kind: GenerationLimit::WorkUnits,
            attempted: budget.work_limit.saturating_add(1),
            geometry_segments: budget.operations,
        });
    }
    sample_bezier(&[points[0], q1, r1, center], tolerance, output, budget)?;
    sample_bezier(&[center, r2, q3, points[3]], tolerance, output, budget)
}

fn simplify(
    points: &[Point],
    distance: f64,
    budget: &mut GenerationBudget,
) -> Result<Vec<Point>, GenerationError> {
    let mut output = Vec::new();
    simplify_points(points, 0, points.len(), distance, &mut output, budget)?;
    Ok(output)
}

fn simplify_points(
    points: &[Point],
    start: usize,
    end: usize,
    epsilon: f64,
    output: &mut Vec<Point>,
    budget: &mut GenerationBudget,
) -> Result<(), GenerationError> {
    budget.work(1)?;
    let first = points[start];
    let last = points[end - 1];
    let mut maximum_distance_squared = 0.0;
    let mut maximum_index = 1;
    for (index, point) in points.iter().enumerate().take(end - 1).skip(start + 1) {
        budget.work(1)?;
        let distance_squared = distance_to_segment_squared(*point, first, last);
        if distance_squared > maximum_distance_squared {
            maximum_distance_squared = distance_squared;
            maximum_index = index;
        }
    }
    if maximum_distance_squared.sqrt() > epsilon {
        simplify_points(points, start, maximum_index + 1, epsilon, output, budget)?;
        simplify_points(points, maximum_index, end, epsilon, output, budget)
    } else {
        budget.work(2)?;
        output.try_reserve(2)?;
        if output.is_empty() {
            output.push(first);
        }
        output.push(last);
        Ok(())
    }
}

fn distance_to_segment_squared(point: Point, start: Point, end: Point) -> f64 {
    let length_squared = square(start.x - end.x) + square(start.y - end.y);
    if length_squared == 0.0 {
        return square(point.x - start.x) + square(point.y - start.y);
    }
    let interpolation = (((point.x - start.x) * (end.x - start.x)
        + (point.y - start.y) * (end.y - start.y))
        / length_squared)
        .clamp(0.0, 1.0);
    let projected = Point {
        x: start.x + (end.x - start.x) * interpolation,
        y: start.y + (end.y - start.y) * interpolation,
    };
    square(point.x - projected.x) + square(point.y - projected.y)
}

fn point_distance(first: Point, second: Point) -> f64 {
    (square(first.x - second.x) + square(first.y - second.y)).sqrt()
}

fn midpoint(first: Point, second: Point) -> Point {
    Point {
        x: first.x + (second.x - first.x) * 0.5,
        y: first.y + (second.y - first.y) * 0.5,
    }
}

/// The vertices `_curveWithOffset` perturbs, in its order: the first point
/// twice, then every later point, the last of them twice again, so a curve
/// over `n >= 2` points draws `2 · (n + 2)` offsets from the seeded stream, two
/// per vertex, `x` before `y`.
fn curve_vertices(points: &[Point]) -> impl Iterator<Item = Point> + '_ {
    points
        .first()
        .copied()
        .into_iter()
        .chain(points.iter().copied())
        .chain(points.last().copied().filter(|_| points.len() > 1))
}

fn curve_stroke(
    points: &[Point],
    offset: f64,
    roughness: f64,
    random: &mut Random,
    operations: &mut Vec<PathOperation>,
    budget: &mut GenerationBudget,
) -> Result<(), GenerationError> {
    if points.is_empty() {
        return Ok(());
    }
    let mut perturbed = Vec::new();
    budget.work(
        u64::try_from(points.len())
            .unwrap_or(u64::MAX)
            .saturating_add(2),
    )?;
    perturbed.try_reserve_exact(points.len().saturating_add(2))?;
    perturbed.extend(curve_vertices(points).map(|point| Point {
        x: point.x + random_offset(offset, roughness, 1.0, random),
        y: point.y + random_offset(offset, roughness, 1.0, random),
    }));
    smooth_curve(&perturbed, operations, budget)
}

/// Draws from `random` exactly what [`curve_stroke`] over the same `points`
/// would, keeping nothing: the reference's `generator.curve` generates the
/// outline before the fill even when `stroke: "none"` then discards it, so a
/// fill drawn without the outline reads the seeded stream from where the
/// outline leaves it (#170).
fn discard_curve_stroke(points: &[Point], offset: f64, roughness: f64, random: &mut Random) {
    for _ in curve_vertices(points) {
        random_offset(offset, roughness, 1.0, random);
        random_offset(offset, roughness, 1.0, random);
    }
}

fn smooth_curve(
    points: &[Point],
    operations: &mut Vec<PathOperation>,
    budget: &mut GenerationBudget,
) -> Result<(), GenerationError> {
    budget.points(points)?;
    if points.len() == 3 {
        budget.push_operation(operations, PathOperation::Move(points[1]))?;
        budget.push_operation(
            operations,
            PathOperation::Cubic {
                control_1: points[1],
                control_2: points[2],
                end: points[2],
            },
        )?;
        return Ok(());
    }
    if points.len() <= 3 {
        return Ok(());
    }
    budget.push_operation(operations, PathOperation::Move(points[1]))?;
    for index in 1..points.len() - 2 {
        let current = points[index];
        let next = points[index + 1];
        budget.push_operation(
            operations,
            PathOperation::Cubic {
                control_1: Point {
                    x: current.x + (next.x - points[index - 1].x) / 6.0,
                    y: current.y + (next.y - points[index - 1].y) / 6.0,
                },
                control_2: Point {
                    x: next.x + (current.x - points[index + 2].x) / 6.0,
                    y: next.y + (current.y - points[index + 2].y) / 6.0,
                },
                end: next,
            },
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn ellipse_drawing(
    width: f64,
    height: f64,
    seed: i32,
    roughness: f64,
    preserve_vertices: bool,
    disable_multi_stroke: bool,
    fill_style: Option<FillStyle>,
    source_stroke_width: f64,
    budget: &mut GenerationBudget,
) -> Result<Drawing, GenerationError> {
    ellipse_drawing_with_curve_fitting(
        width,
        height,
        seed,
        roughness,
        preserve_vertices,
        disable_multi_stroke,
        fill_style,
        source_stroke_width,
        1.0,
        budget,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn circle_drawing(
    diameter: f64,
    seed: i32,
    roughness: f64,
    preserve_vertices: bool,
    disable_multi_stroke: bool,
    fill_style: Option<FillStyle>,
    source_stroke_width: f64,
    budget: &mut GenerationBudget,
) -> Result<Drawing, GenerationError> {
    ellipse_drawing_with_curve_fitting(
        diameter,
        diameter,
        seed,
        roughness,
        preserve_vertices,
        disable_multi_stroke,
        fill_style,
        source_stroke_width,
        0.95,
        budget,
    )
}

#[allow(clippy::too_many_arguments)]
fn ellipse_drawing_with_curve_fitting(
    width: f64,
    height: f64,
    seed: i32,
    roughness: f64,
    preserve_vertices: bool,
    disable_multi_stroke: bool,
    fill_style: Option<FillStyle>,
    source_stroke_width: f64,
    curve_fitting: f64,
    budget: &mut GenerationBudget,
) -> Result<Drawing, GenerationError> {
    let perimeter =
        (std::f64::consts::PI * 2.0 * ((square(width / 2.0) + square(height / 2.0)) / 2.0).sqrt())
            .sqrt();
    let step_count = 9.0_f64.max((9.0 / 200.0_f64.sqrt()) * perimeter).ceil();
    let increment = std::f64::consts::TAU / step_count;
    let center = Point {
        x: width / 2.0,
        y: height / 2.0,
    };
    let mut random = Random::new(seed);
    let base_radius_x = width.abs() / 2.0;
    let base_radius_y = height.abs() / 2.0;
    let radius_x = base_radius_x
        + random_offset(
            base_radius_x * (1.0 - curve_fitting),
            roughness,
            1.0,
            &mut random,
        );
    let radius_y = base_radius_y
        + random_offset(
            base_radius_y * (1.0 - curve_fitting),
            roughness,
            1.0,
            &mut random,
        );
    let overlap = ellipse_overlap(increment, roughness, &mut random);
    let mut operations = Vec::new();
    if roughness == 0.0 {
        core_ellipse_stroke(
            center,
            radius_x,
            radius_y,
            increment / 4.0,
            &mut operations,
            budget,
        )?;
        let fill = match fill_style {
            Some(FillStyle::Solid) => {
                // Rough.js invokes ellipseWithParams again for the solid fill, including
                // both nested overlap random calls even though roughness makes them zero.
                let _overlap = ellipse_overlap(increment, roughness, &mut random);
                let mut fill_operations = Vec::new();
                core_ellipse_stroke(
                    center,
                    radius_x,
                    radius_y,
                    increment / 4.0,
                    &mut fill_operations,
                    budget,
                )?;
                Some(Fill {
                    path: Path {
                        operations: fill_operations,
                    },
                    kind: FillKind::Solid,
                })
            }
            Some(style) => {
                let points =
                    core_ellipse_points(center, radius_x, radius_y, increment / 4.0, budget)?;
                Some(fill_polygon(
                    &points,
                    FillOptions {
                        style,
                        source_stroke_width,
                        roughness,
                        preserve_vertices,
                        disable_multi_stroke,
                        curve_fitting,
                        seed,
                    },
                    &mut random,
                    budget,
                )?)
            }
            None => None,
        };
        return Ok(Drawing {
            outline: Path { operations },
            fill,
            shape: DrawableShape::Ellipse,
            decorations: Vec::new(),
        });
    }
    let mut estimated_points = Vec::new();
    ellipse_stroke(
        center,
        radius_x,
        radius_y,
        increment,
        1.0,
        overlap,
        roughness,
        &mut random,
        &mut operations,
        Some(&mut estimated_points),
        budget,
    )?;
    if !disable_multi_stroke {
        ellipse_stroke(
            center,
            radius_x,
            radius_y,
            increment,
            1.5,
            0.0,
            roughness,
            &mut random,
            &mut operations,
            None,
            budget,
        )?;
    }
    let fill = match fill_style {
        Some(FillStyle::Solid) => {
            let overlap = ellipse_overlap(increment, roughness, &mut random);
            let mut fill_operations = Vec::new();
            ellipse_stroke(
                center,
                radius_x,
                radius_y,
                increment,
                1.0,
                overlap,
                roughness,
                &mut random,
                &mut fill_operations,
                None,
                budget,
            )?;
            if !disable_multi_stroke {
                ellipse_stroke(
                    center,
                    radius_x,
                    radius_y,
                    increment,
                    1.5,
                    0.0,
                    roughness,
                    &mut random,
                    &mut fill_operations,
                    None,
                    budget,
                )?;
            }
            Some(Fill {
                path: Path {
                    operations: fill_operations,
                },
                kind: FillKind::Solid,
            })
        }
        Some(style) => Some(fill_polygon(
            &estimated_points,
            FillOptions {
                style,
                source_stroke_width,
                roughness,
                preserve_vertices,
                disable_multi_stroke,
                curve_fitting,
                seed,
            },
            &mut random,
            budget,
        )?),
        None => None,
    };
    Ok(Drawing {
        outline: Path { operations },
        fill,
        shape: DrawableShape::Ellipse,
        decorations: Vec::new(),
    })
}

fn core_ellipse_points(
    center: Point,
    radius_x: f64,
    radius_y: f64,
    increment: f64,
    budget: &mut GenerationBudget,
) -> Result<Vec<Point>, GenerationError> {
    let mut points = Vec::new();
    let mut angle = 0.0;
    while angle <= std::f64::consts::TAU {
        budget.work(1)?;
        points.try_reserve(1)?;
        points.push(Point {
            x: center.x + radius_x * cos(angle),
            y: center.y + radius_y * sin(angle),
        });
        let next = angle + increment;
        if next <= angle {
            return Err(GenerationError::Limit {
                kind: GenerationLimit::WorkUnits,
                attempted: budget.work_limit.saturating_add(1),
                geometry_segments: budget.operations,
            });
        }
        angle = next;
    }
    Ok(points)
}

fn core_ellipse_stroke(
    center: Point,
    radius_x: f64,
    radius_y: f64,
    increment: f64,
    operations: &mut Vec<PathOperation>,
    budget: &mut GenerationBudget,
) -> Result<(), GenerationError> {
    let point = |angle: f64| Point {
        x: center.x + radius_x * cos(angle),
        y: center.y + radius_y * sin(angle),
    };
    let mut points = Vec::new();
    budget.work(1)?;
    points.try_reserve(1)?;
    points.push(point(-increment));
    let mut angle = 0.0;
    while angle <= std::f64::consts::TAU {
        budget.work(1)?;
        points.try_reserve(1)?;
        points.push(point(angle));
        let next = angle + increment;
        if next <= angle {
            return Err(GenerationError::Limit {
                kind: GenerationLimit::WorkUnits,
                attempted: budget.work_limit.saturating_add(1),
                geometry_segments: budget.operations,
            });
        }
        angle = next;
    }
    budget.work(2)?;
    points.try_reserve(2)?;
    points.push(point(0.0));
    points.push(point(increment));
    smooth_curve(&points, operations, budget)
}

#[allow(clippy::too_many_arguments)]
fn ellipse_stroke(
    center: Point,
    radius_x: f64,
    radius_y: f64,
    increment: f64,
    offset: f64,
    overlap: f64,
    roughness: f64,
    random: &mut Random,
    operations: &mut Vec<PathOperation>,
    mut estimated_points: Option<&mut Vec<Point>>,
    budget: &mut GenerationBudget,
) -> Result<(), GenerationError> {
    let angle_offset = random_offset(0.5, roughness, 1.0, random) - std::f64::consts::FRAC_PI_2;
    let mut points = Vec::new();
    let point = |angle: f64, scale: f64, random: &mut Random| Point {
        x: random_offset(offset, roughness, 1.0, random) + center.x + scale * radius_x * cos(angle),
        y: random_offset(offset, roughness, 1.0, random) + center.y + scale * radius_y * sin(angle),
    };
    budget.work(1)?;
    points.try_reserve(1)?;
    points.push(point(angle_offset - increment, 0.9, random));
    let end_angle = std::f64::consts::TAU + angle_offset - 0.01;
    let mut angle = angle_offset;
    while angle < end_angle {
        budget.work(1)?;
        let point = point(angle, 1.0, random);
        points.try_reserve(1)?;
        points.push(point);
        if let Some(estimated_points) = estimated_points.as_deref_mut() {
            budget.work(1)?;
            estimated_points.try_reserve(1)?;
            estimated_points.push(point);
        }
        let next = angle + increment;
        if next <= angle {
            return Err(GenerationError::Limit {
                kind: GenerationLimit::WorkUnits,
                attempted: budget.work_limit.saturating_add(1),
                geometry_segments: budget.operations,
            });
        }
        angle = next;
    }
    budget.work(3)?;
    points.try_reserve(3)?;
    points.push(point(
        angle_offset + std::f64::consts::TAU + overlap * 0.5,
        1.0,
        random,
    ));
    points.push(point(angle_offset + overlap, 0.98, random));
    points.push(point(angle_offset + overlap * 0.5, 0.9, random));
    smooth_curve(&points, operations, budget)
}

#[allow(clippy::too_many_arguments)]
fn double_bezier_mode(
    start: Point,
    control_1: Point,
    control_2: Point,
    end: Point,
    roughness: f64,
    preserve_vertices: bool,
    disable_multi_stroke: bool,
    random: &mut Random,
    operations: &mut Vec<PathOperation>,
    budget: &mut GenerationBudget,
) -> Result<(), GenerationError> {
    let iterations = if disable_multi_stroke { 1 } else { 2 };
    for (index, extent) in [2.0, 2.3].into_iter().take(iterations).enumerate() {
        let move_point = if index == 0 || preserve_vertices {
            start
        } else {
            Point {
                x: start.x + random_offset(2.0, roughness, 1.0, random),
                y: start.y + random_offset(2.0, roughness, 1.0, random),
            }
        };
        budget.push_operation(operations, PathOperation::Move(move_point))?;
        let endpoint = if preserve_vertices {
            end
        } else {
            Point {
                x: end.x + random_offset(extent, roughness, 1.0, random),
                y: end.y + random_offset(extent, roughness, 1.0, random),
            }
        };
        budget.push_operation(
            operations,
            PathOperation::Cubic {
                control_1: Point {
                    x: control_1.x + random_offset(extent, roughness, 1.0, random),
                    y: control_1.y + random_offset(extent, roughness, 1.0, random),
                },
                control_2: Point {
                    x: control_2.x + random_offset(extent, roughness, 1.0, random),
                    y: control_2.y + random_offset(extent, roughness, 1.0, random),
                },
                end: endpoint,
            },
        )?;
    }
    Ok(())
}

fn random_range(minimum: f64, maximum: f64, roughness: f64, random: &mut Random) -> f64 {
    roughness * (random.next() * (maximum - minimum) + minimum)
}

fn ellipse_overlap(increment: f64, roughness: f64, random: &mut Random) -> f64 {
    let extent = random_range(0.4, 1.0, roughness, random);
    increment * random_range(0.1, extent, roughness, random)
}

#[allow(clippy::too_many_arguments)]
fn double_line_mode(
    start: Point,
    end: Point,
    roughness: f64,
    preserve_vertices: bool,
    disable_multi_stroke: bool,
    filling: bool,
    random: &mut Random,
    operations: &mut Vec<PathOperation>,
    budget: &mut GenerationBudget,
) -> Result<(), GenerationError> {
    line(
        start,
        end,
        roughness,
        preserve_vertices,
        false,
        random,
        operations,
        budget,
    )?;
    if !disable_multi_stroke || filling {
        line(
            start,
            end,
            roughness,
            preserve_vertices,
            true,
            random,
            operations,
            budget,
        )?;
    }
    Ok(())
}

/// `patternFillPolygons` with the filler `getFiller` selects for the style,
/// after `solidFillPolygon` for `solid`. Every pattern filler scans the
/// polygon with `polygonHachureLines` once (twice for cross-hatch) and then
/// turns the scan lines into operations its own way.
fn fill_polygon(
    points: &[Point],
    options: FillOptions,
    random: &mut Random,
    budget: &mut GenerationBudget,
) -> Result<Fill, GenerationError> {
    budget.points(points)?;
    let FillOptions {
        style,
        source_stroke_width: stroke_width,
        roughness,
        preserve_vertices,
        ..
    } = options;
    if matches!(style, FillStyle::Solid) {
        let mut operations = Vec::new();
        if points.len() > 2 {
            let perturb = |point: Point, random: &mut Random| Point {
                x: point.x + random_offset(2.0, roughness, 1.0, random),
                y: point.y + random_offset(2.0, roughness, 1.0, random),
            };
            budget.push_operation(
                &mut operations,
                PathOperation::Move(perturb(points[0], random)),
            )?;
            for point in points.iter().copied().skip(1) {
                budget
                    .push_operation(&mut operations, PathOperation::Line(perturb(point, random)))?;
            }
        }
        return Ok(Fill {
            path: Path { operations },
            kind: FillKind::Solid,
        });
    }

    // Excalidraw sets `hachureGap` to four times the element's stroke width,
    // whatever the stroke style adds to the outline's width; the scan clamps
    // it to 0.1.
    let hachure_gap = stroke_width * 4.0;
    let gap = hachure_gap.max(0.1);
    budget.coordinate(gap)?;
    let mut polygon = Vec::new();
    budget.work(u64::try_from(points.len()).unwrap_or(u64::MAX))?;
    polygon.try_reserve_exact(points.len())?;
    polygon.extend_from_slice(points);
    let mut operations = Vec::new();
    // Each filler consumes the scan lines as the scan yields them, so a fill
    // over the element's ceilings stops before its lines accumulate.
    let fill_line = |start: Point,
                     end: Point,
                     random: &mut Random,
                     operations: &mut Vec<PathOperation>,
                     budget: &mut GenerationBudget| {
        double_line_mode(
            start,
            end,
            roughness,
            preserve_vertices,
            false,
            true,
            random,
            operations,
            budget,
        )
    };
    match style {
        FillStyle::Solid => unreachable!("solid fills return above"),
        FillStyle::Hachure => {
            hachure_lines(
                &mut polygon,
                gap,
                49.0,
                roughness,
                random,
                budget,
                |start, end, random, budget| fill_line(start, end, random, &mut operations, budget),
            )?;
        }
        FillStyle::CrossHatch => {
            for angle in [49.0, 139.0] {
                hachure_lines(
                    &mut polygon,
                    gap,
                    angle,
                    roughness,
                    random,
                    budget,
                    |start, end, random, budget| {
                        fill_line(start, end, random, &mut operations, budget)
                    },
                )?;
            }
        }
        FillStyle::Zigzag => {
            let angle = -41.0_f64.to_radians();
            let delta_x = gap * 0.5 * cos(angle);
            let delta_y = gap * 0.5 * sin(angle);
            hachure_lines(
                &mut polygon,
                gap,
                49.0,
                roughness,
                random,
                budget,
                |start, end, random, budget| {
                    if hypot(end.x - start.x, end.y - start.y) == 0.0 {
                        return Ok(());
                    }
                    budget.work(2)?;
                    let first = Point {
                        x: start.x - delta_x,
                        y: start.y + delta_y,
                    };
                    fill_line(first, end, random, &mut operations, budget)?;
                    let second = Point {
                        x: start.x + delta_x,
                        y: start.y - delta_y,
                    };
                    fill_line(second, end, random, &mut operations, budget)
                },
            )?;
        }
        FillStyle::ZigzagLine => {
            // `ZigZagLineFiller`: `zigzagOffset` is unset, so it equals the
            // gap, and the scan runs at `hachureGap + zigzagOffset`.
            let zigzag_offset = hachure_gap;
            hachure_lines(
                &mut polygon,
                (hachure_gap + zigzag_offset).max(0.1),
                49.0,
                roughness,
                random,
                budget,
                |start, end, random, budget| {
                    zigzag_line(
                        start,
                        end,
                        zigzag_offset,
                        roughness,
                        preserve_vertices,
                        random,
                        &mut operations,
                        budget,
                    )
                },
            )?;
        }
        FillStyle::Dots => {
            // `DotFiller`: the scan runs at `hachureAngle` 0, so its lines are
            // vertical.
            let mut scatter = Random::new(options.seed);
            hachure_lines(
                &mut polygon,
                gap,
                90.0,
                roughness,
                random,
                budget,
                |start, end, random, budget| {
                    dots_on_line(
                        start,
                        end,
                        options,
                        &mut scatter,
                        random,
                        &mut operations,
                        budget,
                    )
                },
            )?;
        }
        FillStyle::Dashed => {
            // `DashedFiller`: `dashOffset` and `dashGap` are unset, so both
            // are the unclamped `hachureGap`.
            hachure_lines(
                &mut polygon,
                gap,
                49.0,
                roughness,
                random,
                budget,
                |start, end, random, budget| {
                    dashed_line(
                        start,
                        end,
                        hachure_gap,
                        hachure_gap,
                        roughness,
                        preserve_vertices,
                        random,
                        &mut operations,
                        budget,
                    )
                },
            )?;
        }
    }
    Ok(Fill {
        path: Path { operations },
        kind: FillKind::Sketch,
    })
}

/// `lineLength`: `Math.sqrt(Math.pow(dx, 2) + Math.pow(dy, 2))`, where V8's
/// `Math.pow(x, 2)` is the one multiplication `x * x`.
fn line_length(start: Point, end: Point) -> f64 {
    (square(start.x - end.x) + square(start.y - end.y)).sqrt()
}

/// The endpoint of a scan line with the smaller x, and the angle of the line
/// as `Math.atan` of its slope from there — how the dashed and zigzag-line
/// fillers walk a line.
fn ordered_line(start: Point, end: Point) -> (Point, f64) {
    let (first, second) = if start.x > end.x {
        (end, start)
    } else {
        (start, end)
    };
    let alpha = atan((second.y - first.y) / (second.x - first.x));
    (first, alpha)
}

/// `DashedFiller.dashedLine`: a scan line is cut into `count` dashes of
/// `offset` length separated by `gap`, centred on the line, each drawn as a
/// double fill line from its start to its end.
#[allow(clippy::too_many_arguments)]
fn dashed_line(
    start: Point,
    end: Point,
    offset: f64,
    gap: f64,
    roughness: f64,
    preserve_vertices: bool,
    random: &mut Random,
    operations: &mut Vec<PathOperation>,
    budget: &mut GenerationBudget,
) -> Result<(), GenerationError> {
    let length = line_length(start, end);
    let count = (length / (offset + gap)).floor();
    let start_offset = (length + gap - count * (offset + gap)) / 2.0;
    let (first, alpha) = ordered_line(start, end);
    let cos_alpha = cos(alpha);
    let sin_alpha = sin(alpha);
    for index in 0..dash_count(count) {
        budget.work(1)?;
        let index = index as f64;
        let dash_start = index * (offset + gap);
        let dash_end = dash_start + offset;
        let dash = |distance: f64| Point {
            x: first.x + distance * cos_alpha + start_offset * cos_alpha,
            y: first.y + distance * sin_alpha + start_offset * sin_alpha,
        };
        double_line_mode(
            dash(dash_start),
            dash(dash_end),
            roughness,
            preserve_vertices,
            false,
            true,
            random,
            operations,
            budget,
        )?;
    }
    Ok(())
}

/// `ZigZagLineFiller.zigzagLines`: a scan line is walked in steps of twice
/// the zigzag offset, `Math.round(length / (2 * zo))` of them, and each step
/// draws two double fill lines through a middle point raised at 45 degrees.
#[allow(clippy::too_many_arguments)]
fn zigzag_line(
    start: Point,
    end: Point,
    zigzag_offset: f64,
    roughness: f64,
    preserve_vertices: bool,
    random: &mut Random,
    operations: &mut Vec<PathOperation>,
    budget: &mut GenerationBudget,
) -> Result<(), GenerationError> {
    let length = line_length(start, end);
    let count = ecmascript_round(length / (2.0 * zigzag_offset));
    let (first, alpha) = ordered_line(start, end);
    let cos_alpha = cos(alpha);
    let sin_alpha = sin(alpha);
    let diagonal = (2.0 * square(zigzag_offset)).sqrt();
    let raised = alpha + std::f64::consts::FRAC_PI_4;
    let cos_raised = cos(raised);
    let sin_raised = sin(raised);
    for index in 0..dash_count(count) {
        budget.work(1)?;
        let step_start = (index * 2) as f64 * zigzag_offset;
        let step_end = ((index + 1) * 2) as f64 * zigzag_offset;
        let step = Point {
            x: first.x + step_start * cos_alpha,
            y: first.y + step_start * sin_alpha,
        };
        let step_end = Point {
            x: first.x + step_end * cos_alpha,
            y: first.y + step_end * sin_alpha,
        };
        let middle = Point {
            x: step.x + diagonal * cos_raised,
            y: step.y + diagonal * sin_raised,
        };
        double_line_mode(
            step,
            middle,
            roughness,
            preserve_vertices,
            false,
            true,
            random,
            operations,
            budget,
        )?;
        double_line_mode(
            middle,
            step_end,
            roughness,
            preserve_vertices,
            false,
            true,
            random,
            operations,
            budget,
        )?;
    }
    Ok(())
}

/// The iteration count a JavaScript `for (let i = 0; i < count; i++)` runs
/// for a numeric `count`: none when it is not positive (or NaN).
fn dash_count(count: f64) -> u64 {
    if count > 0.0 {
        count.ceil() as u64
    } else {
        0
    }
}

/// `DotFiller.dotsOnLines`: along one vertical scan line, `ceil(length /
/// gap) - 1` dots a gap apart, centred on the line's remainder, each a Rough.js
/// ellipse of `fillWeight` diameter whose centre is scattered by up to a
/// quarter gap in each axis.
///
/// The reference scatters the centres with the unseeded `Math.random()`, so
/// its own output for this filler is not reproducible from one export to the
/// next. Revision 1 draws the scatter from a second Rough.js `Random` seeded
/// with the element seed, separate from the seeded stream the options carry:
/// the stream the reference consumes for the dot ellipses is untouched, so the
/// ellipses are the reference's own, and the centres are one deterministic
/// sample of the reference's jitter (#137).
fn dots_on_line(
    start: Point,
    end: Point,
    options: FillOptions,
    scatter: &mut Random,
    random: &mut Random,
    operations: &mut Vec<PathOperation>,
    budget: &mut GenerationBudget,
) -> Result<(), GenerationError> {
    let gap = (options.source_stroke_width * 4.0).max(0.1);
    let fill_weight = options.source_stroke_width / 2.0;
    let radius = gap / 4.0;
    let length = line_length(start, end);
    let count = (length / gap).ceil() - 1.0;
    let offset = length - count * gap;
    let x = (start.x + end.x) / 2.0 - gap / 4.0;
    let min_y = start.y.min(end.y);
    for index in 0..dash_count(count) {
        budget.work(1)?;
        let y = min_y + offset + index as f64 * gap;
        let center = Point {
            x: (x - radius) + scatter.next() * 2.0 * radius,
            y: (y - radius) + scatter.next() * 2.0 * radius,
        };
        dot_ellipse(center, fill_weight, options, random, operations, budget)?;
    }
    Ok(())
}

/// Rough.js `ellipse(x, y, width, height, o)` for one dot: the ellipse
/// parameters under the element's `curveFitting`, then the sketched stroke
/// (double unless `disableMultiStroke`), or the single core stroke at
/// roughness 0 — the same sequence `ellipse_drawing` draws for an ellipse
/// element's outline, consuming the seeded generator identically.
fn dot_ellipse(
    center: Point,
    diameter: f64,
    options: FillOptions,
    random: &mut Random,
    operations: &mut Vec<PathOperation>,
    budget: &mut GenerationBudget,
) -> Result<(), GenerationError> {
    let perimeter = (std::f64::consts::PI
        * 2.0
        * ((square(diameter / 2.0) + square(diameter / 2.0)) / 2.0).sqrt())
    .sqrt();
    let step_count = 9.0_f64.max((9.0 / 200.0_f64.sqrt()) * perimeter).ceil();
    let increment = std::f64::consts::TAU / step_count;
    let base_radius = (diameter / 2.0).abs();
    let curve_fit_randomness = 1.0 - options.curve_fitting;
    let radius_x = base_radius
        + random_offset(
            base_radius * curve_fit_randomness,
            options.roughness,
            1.0,
            random,
        );
    let radius_y = base_radius
        + random_offset(
            base_radius * curve_fit_randomness,
            options.roughness,
            1.0,
            random,
        );
    let overlap = ellipse_overlap(increment, options.roughness, random);
    if options.roughness == 0.0 {
        return core_ellipse_stroke(
            center,
            radius_x,
            radius_y,
            increment / 4.0,
            operations,
            budget,
        );
    }
    ellipse_stroke(
        center,
        radius_x,
        radius_y,
        increment,
        1.0,
        overlap,
        options.roughness,
        random,
        operations,
        None,
        budget,
    )?;
    if !options.disable_multi_stroke {
        ellipse_stroke(
            center,
            radius_x,
            radius_y,
            increment,
            1.5,
            0.0,
            options.roughness,
            random,
            operations,
            None,
            budget,
        )?;
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct Edge {
    minimum_y: f64,
    maximum_y: f64,
    x: f64,
    inverse_slope: f64,
}

fn hachure_lines(
    points: &mut [Point],
    gap: f64,
    angle: f64,
    roughness: f64,
    random: &mut Random,
    budget: &mut GenerationBudget,
    mut line: impl FnMut(
        Point,
        Point,
        &mut Random,
        &mut GenerationBudget,
    ) -> Result<(), GenerationError>,
) -> Result<(), GenerationError> {
    budget.coordinate(gap)?;
    let radians = angle.to_radians();
    let (sin, cos) = sin_cos(radians);
    for point in &mut *points {
        *point = Point {
            x: point.x * cos - point.y * sin,
            y: point.x * sin + point.y * cos,
        };
        budget.point(*point)?;
    }
    let mut rotated = Vec::new();
    budget.work(
        u64::try_from(points.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1),
    )?;
    rotated.try_reserve_exact(points.len().saturating_add(1))?;
    rotated.extend_from_slice(points);
    if let (Some(first), Some(last)) = (rotated.first().copied(), rotated.last().copied()) {
        if first.x != last.x || first.y != last.y {
            rotated.push(first);
        }
    }

    let mut edges = Vec::new();
    budget.work(u64::try_from(rotated.len()).unwrap_or(u64::MAX))?;
    edges.try_reserve_exact(rotated.len().saturating_sub(1))?;
    for pair in rotated.windows(2) {
        let first = pair[0];
        let second = pair[1];
        if first.y != second.y {
            let minimum_y = first.y.min(second.y);
            edges.push(Edge {
                minimum_y,
                maximum_y: first.y.max(second.y),
                x: if minimum_y == first.y {
                    first.x
                } else {
                    second.x
                },
                inverse_slope: (second.x - first.x) / (second.y - first.y),
            });
        }
    }
    edges.sort_by(|first, second| {
        first
            .minimum_y
            .total_cmp(&second.minimum_y)
            .then(first.x.total_cmp(&second.x))
            .then(first.maximum_y.total_cmp(&second.maximum_y))
    });
    let Some(first_edge) = edges.first() else {
        rotate_points(points, -angle);
        return Ok(());
    };
    let mut y = first_edge.minimum_y;
    let step = if roughness >= 1.0 && random.next() > 0.7 {
        gap
    } else {
        1.0
    };
    let mut active: Vec<Edge> = Vec::new();
    let mut iteration = 0_u64;
    while !active.is_empty() || !edges.is_empty() {
        budget.work(1)?;
        let count = edges.partition_point(|edge| edge.minimum_y <= y);
        budget.work(u64::try_from(count).unwrap_or(u64::MAX))?;
        active.try_reserve(count)?;
        active.extend(edges.drain(..count));
        active.retain(|edge| edge.maximum_y > y);
        budget.work(u64::try_from(active.len()).unwrap_or(u64::MAX))?;
        active.sort_by(|first, second| first.x.total_cmp(&second.x));
        if step != 1.0 || iteration as f64 % gap == 0.0 {
            for pair in active.as_chunks::<2>().0 {
                let start = rotate_point(
                    Point {
                        x: ecmascript_round(pair[0].x),
                        y,
                    },
                    -angle,
                );
                let end = rotate_point(
                    Point {
                        x: ecmascript_round(pair[1].x),
                        y,
                    },
                    -angle,
                );
                budget.point(start)?;
                budget.point(end)?;
                budget.work(1)?;
                line(start, end, random, budget)?;
            }
        }
        let next_y = y + step;
        if next_y <= y {
            return Err(GenerationError::Limit {
                kind: GenerationLimit::WorkUnits,
                attempted: budget.work_limit.saturating_add(1),
                geometry_segments: budget.operations,
            });
        }
        y = next_y;
        for edge in &mut active {
            edge.x += step * edge.inverse_slope;
            // Only active intersections are exposed to the next scanline.
            if edge.maximum_y > y {
                budget.coordinate(edge.x)?;
            }
        }
        iteration = iteration.saturating_add(1);
    }
    rotate_points(points, -angle);
    budget.points(points)?;
    Ok(())
}

fn rotate_points(points: &mut [Point], degrees: f64) {
    let radians = degrees.to_radians();
    for point in points {
        *point = Point {
            x: point.x * cos(radians) - point.y * sin(radians),
            y: point.x * sin(radians) + point.y * cos(radians),
        };
    }
}

fn rotate_point(point: Point, degrees: f64) -> Point {
    let radians = degrees.to_radians();
    Point {
        x: point.x * cos(radians) - point.y * sin(radians),
        y: point.x * sin(radians) + point.y * cos(radians),
    }
}

#[allow(clippy::too_many_arguments)]
fn line(
    start: Point,
    end: Point,
    roughness: f64,
    preserve_vertices: bool,
    overlay: bool,
    random: &mut Random,
    operations: &mut Vec<PathOperation>,
    budget: &mut GenerationBudget,
) -> Result<(), GenerationError> {
    budget.point(start)?;
    budget.point(end)?;
    const MAX_RANDOMNESS_OFFSET: f64 = 2.0;
    const BOWING: f64 = 1.0;

    let delta_x = end.x - start.x;
    let delta_y = end.y - start.y;
    let distance_x = start.x - end.x;
    let distance_y = start.y - end.y;
    let length_squared = distance_x * distance_x + distance_y * distance_y;
    let length = length_squared.sqrt();
    let roughness_gain = if length < 200.0 {
        1.0
    } else if length > 500.0 {
        0.4
    } else {
        -0.0016668 * length + 1.233334
    };
    let mut randomness_offset = MAX_RANDOMNESS_OFFSET;
    if randomness_offset * randomness_offset * 100.0 > length_squared {
        randomness_offset = length / 10.0;
    }
    let half_offset = randomness_offset / 2.0;
    let diverge_point = 0.2 + random.next() * 0.2;

    let mut mid_displacement_x = BOWING * MAX_RANDOMNESS_OFFSET;
    mid_displacement_x *= delta_y;
    mid_displacement_x /= 200.0;
    let mut mid_displacement_y = BOWING * MAX_RANDOMNESS_OFFSET;
    mid_displacement_y *= -delta_x;
    mid_displacement_y /= 200.0;
    mid_displacement_x = random_offset(mid_displacement_x, roughness, roughness_gain, random);
    mid_displacement_y = random_offset(mid_displacement_y, roughness, roughness_gain, random);

    let offset = if overlay {
        half_offset
    } else {
        randomness_offset
    };
    let moved_start = if preserve_vertices {
        start
    } else {
        Point {
            x: start.x + random_offset(offset, roughness, roughness_gain, random),
            y: start.y + random_offset(offset, roughness, roughness_gain, random),
        }
    };
    budget.push_operation(operations, PathOperation::Move(moved_start))?;
    let control_1 = Point {
        x: control_coordinate(
            mid_displacement_x,
            start.x,
            delta_x,
            diverge_point,
            offset,
            roughness,
            roughness_gain,
            random,
        ),
        y: control_coordinate(
            mid_displacement_y,
            start.y,
            delta_y,
            diverge_point,
            offset,
            roughness,
            roughness_gain,
            random,
        ),
    };
    let doubled_delta_x = 2.0 * delta_x;
    let doubled_delta_y = 2.0 * delta_y;
    let control_2 = Point {
        x: control_coordinate(
            mid_displacement_x,
            start.x,
            doubled_delta_x,
            diverge_point,
            offset,
            roughness,
            roughness_gain,
            random,
        ),
        y: control_coordinate(
            mid_displacement_y,
            start.y,
            doubled_delta_y,
            diverge_point,
            offset,
            roughness,
            roughness_gain,
            random,
        ),
    };
    let endpoint = if preserve_vertices {
        end
    } else {
        Point {
            x: end.x + random_offset(offset, roughness, roughness_gain, random),
            y: end.y + random_offset(offset, roughness, roughness_gain, random),
        }
    };
    budget.push_operation(
        operations,
        PathOperation::Cubic {
            control_1,
            control_2,
            end: endpoint,
        },
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn control_coordinate(
    middle_displacement: f64,
    start: f64,
    delta: f64,
    diverge_point: f64,
    randomness_offset: f64,
    roughness: f64,
    roughness_gain: f64,
    random: &mut Random,
) -> f64 {
    let mut coordinate = middle_displacement + start;
    coordinate += delta * diverge_point;
    coordinate + random_offset(randomness_offset, roughness, roughness_gain, random)
}

fn random_offset(extent: f64, roughness: f64, roughness_gain: f64, random: &mut Random) -> f64 {
    let minimum = -extent;
    let maximum = extent;
    let span = maximum - minimum;
    let shifted = random.next() * span + minimum;
    let scaled_roughness = roughness * roughness_gain;
    scaled_roughness * shifted
}

struct Random {
    seed: i32,
}

impl Random {
    const fn new(seed: i32) -> Self {
        Self { seed }
    }

    fn next(&mut self) -> f64 {
        self.seed = 48_271_i32.wrapping_mul(self.seed);
        f64::from((self.seed as u32) & 0x7fff_ffff) / 2_147_483_648.0
    }
}
// Focused ports of Rough.js 4.6.4 and its points-on-curve/path dependencies.
// Copyright (c) 2019-2020 Preet Shihn, licensed under MIT. See the bundled
// Rough.js and points-on-curve license notices.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn architect_solid_ellipse_consumes_both_overlap_pairs() {
        let mut random = Random::new(43);
        assert_eq!(ellipse_overlap(0.25, 0.0, &mut random), 0.0);
        assert_eq!(ellipse_overlap(0.25, 0.0, &mut random), 0.0);
        assert_eq!(random.seed, -610_782_869);
    }
}
