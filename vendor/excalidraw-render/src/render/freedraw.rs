// Perfect Freehand 1.2.0 compatibility port. The upstream implementation is
// MIT licensed; its notice is included in the renderer license bundle.

use super::geometry::Point;
#[cfg(any(feature = "png", feature = "pdf"))]
use super::number_format::ecmascript_number;
use super::rough::{GenerationBudget, GenerationError};
#[cfg(any(feature = "png", feature = "pdf"))]
use super::FormatEncodeError;
use crate::normalization::NormalizedFreedraw;
use crate::portable_math::{hypot, sin, sin_cos, square};
use std::collections::TryReserveError;

const RATE_OF_PRESSURE_CHANGE: f64 = 0.275;
const FIXED_PI: f64 = std::f64::consts::PI + 0.0001;

#[derive(Debug, PartialEq)]
pub(super) struct Path {
    pub(super) operations: Vec<PathOperation>,
}

/// A path with its coordinates reduced as the raster formats draw it.
#[cfg(any(feature = "png", feature = "pdf"))]
#[derive(Debug, PartialEq)]
pub(super) struct ProjectedPath {
    pub(super) operations: Vec<PathOperation>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum PathOperation {
    Move(Point),
    Quadratic { control: Point, end: Point },
    Line(Point),
    Close,
}

impl Path {
    pub(super) fn operation_count(&self) -> usize {
        self.operations.len()
    }
}

#[cfg(feature = "png")]
fn operation_bounds(operations: &[PathOperation]) -> Option<(f64, f64, f64, f64)> {
    let mut current = None;
    let mut bounds = None;
    for operation in operations {
        match *operation {
            PathOperation::Move(point) | PathOperation::Line(point) => {
                include_point(&mut bounds, point);
                current = Some(point);
            }
            PathOperation::Quadratic { control, end } => {
                let start = current?;
                include_quadratic_bounds(&mut bounds, start, control, end);
                current = Some(end);
            }
            PathOperation::Close => {}
        }
    }
    bounds
}

#[cfg(any(feature = "png", feature = "pdf"))]
impl ProjectedPath {
    #[cfg(feature = "png")]
    pub(super) fn bounds(&self) -> Option<(f64, f64, f64, f64)> {
        operation_bounds(&self.operations)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }
}

#[derive(Clone, Copy)]
struct StrokePoint {
    point: Point,
    pressure: f64,
    vector: Point,
    distance: f64,
    running_length: f64,
}

pub(super) fn generate(
    freedraw: &NormalizedFreedraw,
    stroke_width: f64,
    budget: &mut GenerationBudget,
) -> Result<Path, GenerationError> {
    let mut input = Vec::new();
    let input_len = if !freedraw.simulate_pressure && freedraw.points.is_empty() {
        1
    } else {
        freedraw.points.len()
    };
    input.try_reserve_exact(input_len)?;
    if !freedraw.simulate_pressure && freedraw.points.is_empty() {
        input.push([0.0, 0.0, 0.5]);
    } else {
        input.extend(freedraw.points.iter().enumerate().map(|(index, point)| {
            [
                point[0],
                point[1],
                if freedraw.simulate_pressure {
                    -1.0
                } else {
                    freedraw.pressures.get(index).copied().unwrap_or(-1.0)
                },
            ]
        }));
    }
    budget.work(u64::try_from(input.len()).unwrap_or(u64::MAX))?;
    let size = stroke_width * 4.25;
    budget.coordinate(size)?;
    let stroke_points = stroke_points(&input, size, false)?;
    for point in &stroke_points {
        budget.point(point.point)?;
    }
    budget.work(u64::try_from(stroke_points.len()).unwrap_or(u64::MAX))?;
    let outline = stroke_outline(&stroke_points, size, freedraw.simulate_pressure, false)?;
    for point in &outline {
        budget.point(*point)?;
    }
    path_from_outline(&outline, budget)
}

/// The Rough.js background fill of a freedraw loop. `preserve_vertices` is the
/// element's raw `roughness < 2`, the value the reference's
/// `generateRoughOptions(element)` gives the curve it fills with (#158).
#[allow(clippy::too_many_arguments)]
pub(super) fn background_fill(
    points: &[[f64; 2]],
    seed: i32,
    roughness: f64,
    preserve_vertices: bool,
    disable_multi_stroke: bool,
    fill_style: super::rough::FillStyle,
    stroke_width: f64,
    budget: &mut GenerationBudget,
) -> Result<Option<super::rough::Fill>, GenerationError> {
    const LOOP_THRESHOLD: f64 = 8.0;
    let Some((first, last)) = points.first().zip(points.last()) else {
        return Ok(None);
    };
    if points.len() < 3 || hypot(first[0] - last[0], first[1] - last[1]) > LOOP_THRESHOLD {
        return Ok(None);
    }
    let simplified = simplify(points, 0.75, budget)?;
    super::rough::curve_fill(
        &simplified,
        seed,
        roughness,
        preserve_vertices,
        disable_multi_stroke,
        fill_style,
        stroke_width,
        budget,
    )
    .map(Some)
}

fn simplify(
    points: &[[f64; 2]],
    epsilon: f64,
    budget: &mut GenerationBudget,
) -> Result<Vec<Point>, GenerationError> {
    enum Task {
        Segment(usize, usize),
        EmitEnd(usize),
    }
    let mut output = Vec::new();
    output.try_reserve_exact(points.len())?;
    let mut tasks = Vec::new();
    tasks.try_reserve(points.len().saturating_mul(2))?;
    tasks.push(Task::Segment(0, points.len()));
    while let Some(task) = tasks.pop() {
        match task {
            Task::EmitEnd(index) => output.push(point(points[index][0], points[index][1])),
            Task::Segment(start, end) => {
                let segment_start = point(points[start][0], points[start][1]);
                let segment_end = point(points[end - 1][0], points[end - 1][1]);
                let mut maximum_distance = 0.0;
                let mut maximum_index = 0;
                for (index, value) in points.iter().enumerate().take(end - 1).skip(start + 1) {
                    budget.work(1)?;
                    let distance = distance_to_segment_squared(
                        point(value[0], value[1]),
                        segment_start,
                        segment_end,
                    );
                    if distance > maximum_distance {
                        maximum_distance = distance;
                        maximum_index = index;
                    }
                }
                if maximum_distance.sqrt() > epsilon {
                    tasks.push(Task::Segment(maximum_index, end));
                    tasks.push(Task::Segment(start, maximum_index + 1));
                } else {
                    if output.is_empty() {
                        output.try_reserve(1)?;
                        output.push(segment_start);
                    }
                    tasks.push(Task::EmitEnd(end - 1));
                }
            }
        }
    }
    Ok(output)
}

fn distance_to_segment_squared(value: Point, start: Point, end: Point) -> f64 {
    let length_squared = distance_squared(start, end);
    if length_squared == 0.0 {
        return distance_squared(value, start);
    }
    let scalar = (((value.x - start.x) * (end.x - start.x)
        + (value.y - start.y) * (end.y - start.y))
        / length_squared)
        .clamp(0.0, 1.0);
    distance_squared(value, lerp(start, end, scalar))
}

fn stroke_points(
    input: &[[f64; 3]],
    size: f64,
    complete: bool,
) -> Result<Vec<StrokePoint>, TryReserveError> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let mut points = Vec::new();
    match input.len() {
        1 => {
            points.try_reserve_exact(2)?;
            points.push(input[0]);
            points.push([input[0][0] + 1.0, input[0][1] + 1.0, input[0][2]]);
        }
        2 => {
            points.try_reserve_exact(5)?;
            points.push(input[0]);
            for index in 1..5 {
                let t = f64::from(index) / 4.0;
                points.push([
                    input[0][0] + (input[1][0] - input[0][0]) * t,
                    input[0][1] + (input[1][1] - input[0][1]) * t,
                    input[0][2] + (input[1][2] - input[0][2]) * t,
                ]);
            }
        }
        _ => {
            points.try_reserve_exact(input.len())?;
            points.extend_from_slice(input);
        }
    }

    let mut output = Vec::new();
    output.try_reserve_exact(points.len())?;
    output.push(StrokePoint {
        point: point(points[0][0], points[0][1]),
        pressure: if points[0][2] >= 0.0 {
            points[0][2]
        } else {
            0.25
        },
        vector: point(1.0, 1.0),
        distance: 0.0,
        running_length: 0.0,
    });
    let interpolation = 0.15 + (1.0 - 0.5) * 0.85;
    let mut reached_minimum = false;
    let mut running_length = 0.0;
    let max = points.len() - 1;
    for (index, input_point) in points.iter().enumerate().skip(1) {
        let previous = *output.last().expect("stroke always has its first point");
        let next = if complete && index == max {
            point(input_point[0], input_point[1])
        } else {
            lerp(
                previous.point,
                point(input_point[0], input_point[1]),
                interpolation,
            )
        };
        if equal(previous.point, next) {
            continue;
        }
        let distance = distance(next, previous.point);
        running_length += distance;
        if index < max && !reached_minimum {
            if running_length < size {
                continue;
            }
            reached_minimum = true;
        }
        output.push(StrokePoint {
            point: next,
            pressure: if input_point[2] >= 0.0 {
                input_point[2]
            } else {
                0.5
            },
            vector: unit(sub(previous.point, next)),
            distance,
            running_length,
        });
    }
    output[0].vector = output.get(1).map_or(point(0.0, 0.0), |next| next.vector);
    Ok(output)
}

fn stroke_outline(
    points: &[StrokePoint],
    size: f64,
    simulate_pressure: bool,
    complete: bool,
) -> Result<Vec<Point>, GenerationError> {
    if points.is_empty() || size <= 0.0 {
        return Ok(Vec::new());
    }
    let total_length = points.last().map_or(0.0, |point| point.running_length);
    let minimum_distance = square(size * 0.5);
    let mut left = Vec::new();
    let mut right = Vec::new();
    let side_capacity = points
        .len()
        .checked_mul(14)
        .ok_or(GenerationError::Memory)?;
    left.try_reserve_exact(side_capacity)?;
    right.try_reserve_exact(side_capacity)?;

    let mut previous_pressure = points
        .iter()
        .take(10)
        .fold(points[0].pressure, |acc, current| {
            let mut pressure = current.pressure;
            if simulate_pressure {
                let speed = (current.distance / size).min(1.0);
                let rate = (1.0 - speed).min(1.0);
                pressure = (acc + (rate - acc) * (speed * RATE_OF_PRESSURE_CHANGE)).min(1.0);
            }
            (acc + pressure) / 2.0
        });
    let mut radius = stroke_radius(size, points.last().unwrap().pressure);
    let mut first_radius = None;
    let mut previous_vector = points[0].vector;
    let mut previous_left = points[0].point;
    let mut previous_right = previous_left;
    let mut temporary_left = previous_left;
    let mut temporary_right = previous_right;
    let mut previous_sharp = false;

    for (index, current) in points.iter().enumerate() {
        if index < points.len() - 1 && total_length - current.running_length < 3.0 {
            continue;
        }
        let mut pressure = current.pressure;
        if simulate_pressure {
            let speed = (current.distance / size).min(1.0);
            let rate = (1.0 - speed).min(1.0);
            pressure = (previous_pressure
                + (rate - previous_pressure) * (speed * RATE_OF_PRESSURE_CHANGE))
                .min(1.0);
        }
        radius = stroke_radius(size, pressure).max(0.01);
        first_radius.get_or_insert(radius);
        let next_vector = points
            .get(index + 1)
            .map_or(current.vector, |next| next.vector);
        let next_dot = dot(current.vector, next_vector);
        let current_sharp = dot(current.vector, previous_vector) < 0.0 && !previous_sharp;
        let next_sharp = next_dot < 0.0;
        if current_sharp || next_sharp {
            let offset = mul(perpendicular(previous_vector), radius);
            let mut t = 0.0;
            while t <= 1.0 {
                temporary_left = rotate(sub(current.point, offset), current.point, FIXED_PI * t);
                left.push(temporary_left);
                temporary_right = rotate(add(current.point, offset), current.point, -FIXED_PI * t);
                right.push(temporary_right);
                t += 1.0 / 13.0;
            }
            previous_left = temporary_left;
            previous_right = temporary_right;
            if next_sharp {
                previous_sharp = true;
            }
            continue;
        }
        previous_sharp = false;
        if index == points.len() - 1 {
            let offset = mul(perpendicular(current.vector), radius);
            left.push(sub(current.point, offset));
            right.push(add(current.point, offset));
            continue;
        }
        let offset = mul(
            perpendicular(lerp(next_vector, current.vector, next_dot)),
            radius,
        );
        temporary_left = sub(current.point, offset);
        if index <= 1 || distance_squared(previous_left, temporary_left) > minimum_distance {
            left.push(temporary_left);
            previous_left = temporary_left;
        }
        temporary_right = add(current.point, offset);
        if index <= 1 || distance_squared(previous_right, temporary_right) > minimum_distance {
            right.push(temporary_right);
            previous_right = temporary_right;
        }
        previous_pressure = pressure;
        previous_vector = current.vector;
    }

    let first_point = points[0].point;
    let last_point = points
        .get(1..)
        .and_then(|tail| tail.last())
        .map_or(add(first_point, point(1.0, 1.0)), |last| last.point);
    if points.len() == 1 && complete {
        let start = project_point_distance(
            first_point,
            unit(perpendicular(sub(first_point, last_point))),
            -first_radius.unwrap_or(radius),
        );
        let mut dot_points = Vec::new();
        dot_points.try_reserve_exact(13)?;
        let mut t = 1.0 / 13.0;
        while t <= 1.0 {
            dot_points.push(rotate(start, first_point, FIXED_PI * 2.0 * t));
            t += 1.0 / 13.0;
        }
        return Ok(dot_points);
    }

    let mut start_cap = Vec::new();
    if points.len() > 1 {
        start_cap.try_reserve_exact(13)?;
        let mut t = 1.0 / 13.0;
        while t <= 1.0 {
            start_cap.push(rotate(right[0], first_point, FIXED_PI * t));
            t += 1.0 / 13.0;
        }
    }
    let mut end_cap = Vec::new();
    if points.len() > 1 {
        end_cap.try_reserve_exact(29)?;
        let direction = perpendicular(negate(points.last().unwrap().vector));
        let start = project_point_distance(last_point, direction, radius);
        let mut t = 1.0 / 29.0;
        while t < 1.0 {
            end_cap.push(rotate(start, last_point, FIXED_PI * 3.0 * t));
            t += 1.0 / 29.0;
        }
    }
    let capacity = left
        .len()
        .saturating_add(end_cap.len())
        .saturating_add(right.len())
        .saturating_add(start_cap.len());
    let mut outline = Vec::new();
    outline.try_reserve_exact(capacity)?;
    outline.extend(left);
    outline.extend(end_cap);
    outline.extend(right.into_iter().rev());
    outline.extend(start_cap);
    Ok(outline)
}

fn path_from_outline(
    outline: &[Point],
    budget: &mut GenerationBudget,
) -> Result<Path, GenerationError> {
    if outline.is_empty() {
        return Ok(Path {
            operations: Vec::new(),
        });
    }
    let operation_count = outline.len().saturating_add(3);
    let mut operations = Vec::new();
    operations.try_reserve_exact(operation_count)?;
    let first = outline[0];
    budget.operation()?;
    operations.push(PathOperation::Move(first));
    for (index, control) in outline.iter().copied().enumerate() {
        let next = outline.get(index + 1).copied().unwrap_or(outline[0]);
        budget.operation()?;
        operations.push(PathOperation::Quadratic {
            control,
            end: point((control.x + next.x) / 2.0, (control.y + next.y) / 2.0),
        });
    }
    budget.operation()?;
    operations.push(PathOperation::Line(first));
    budget.operation()?;
    operations.push(PathOperation::Close);
    Ok(Path { operations })
}

#[cfg(any(feature = "png", feature = "pdf"))]
pub(super) fn project(path: Path) -> Result<ProjectedPath, FormatEncodeError> {
    let mut operations = Vec::new();
    operations
        .try_reserve_exact(path.operations.len())
        .map_err(|_| FormatEncodeError::MemoryUnavailable {
            subject_kind: "format_plan",
            subject: "freedraw_path",
        })?;
    for operation in path.operations {
        match operation {
            PathOperation::Move(point) => {
                operations.push(PathOperation::Move(projected_point(point)?));
            }
            PathOperation::Quadratic { control, end } => {
                operations.push(PathOperation::Quadratic {
                    control: projected_point(control)?,
                    end: projected_point(end)?,
                });
            }
            PathOperation::Line(point) => {
                operations.push(PathOperation::Line(projected_point(point)?));
            }
            PathOperation::Close => operations.push(PathOperation::Close),
        }
    }
    Ok(ProjectedPath { operations })
}

#[cfg(any(feature = "png", feature = "pdf"))]
fn push_reduced_number(output: &mut String, value: f64) -> Result<(), FormatEncodeError> {
    if !value.is_finite() {
        return Err(FormatEncodeError::Encoding(
            "freedraw path contains a non-finite coordinate".to_owned(),
        ));
    }
    let token = ecmascript_number(value)?;
    if let Some(decimal) = token.find('.') {
        let exponent = token[decimal + 1..]
            .find(['e', 'E'])
            .map(|offset| decimal + 1 + offset)
            .unwrap_or(token.len());
        let end = (decimal + 3).min(exponent);
        output.push_str(&token[..end]);
    } else {
        output.push_str(&token);
    }
    Ok(())
}

#[cfg(any(feature = "png", feature = "pdf"))]
fn projected_point(value: Point) -> Result<Point, FormatEncodeError> {
    Ok(point(
        projected_number(value.x)?,
        projected_number(value.y)?,
    ))
}

#[cfg(any(feature = "png", feature = "pdf"))]
fn projected_number(value: f64) -> Result<f64, FormatEncodeError> {
    let mut token = String::new();
    token
        .try_reserve(32)
        .map_err(|_| FormatEncodeError::MemoryUnavailable {
            subject_kind: "format_plan",
            subject: "freedraw_coordinate",
        })?;
    push_reduced_number(&mut token, value)?;
    token.parse().map_err(|_| {
        FormatEncodeError::Encoding("freedraw coordinate reduction is invalid".to_owned())
    })
}

#[cfg(feature = "png")]
fn include_point(bounds: &mut Option<(f64, f64, f64, f64)>, point: Point) {
    *bounds = Some(
        bounds.map_or((point.x, point.y, point.x, point.y), |current| {
            (
                current.0.min(point.x),
                current.1.min(point.y),
                current.2.max(point.x),
                current.3.max(point.y),
            )
        }),
    );
}

#[cfg(feature = "png")]
fn include_quadratic_bounds(
    bounds: &mut Option<(f64, f64, f64, f64)>,
    start: Point,
    control: Point,
    end: Point,
) {
    include_point(bounds, start);
    include_point(bounds, end);
    for (start, control, end, x_axis) in [
        (start.x, control.x, end.x, true),
        (start.y, control.y, end.y, false),
    ] {
        let denominator = start - 2.0 * control + end;
        if denominator == 0.0 {
            continue;
        }
        let t = (start - control) / denominator;
        if (0.0..1.0).contains(&t) {
            let inverse = 1.0 - t;
            let value = inverse * inverse * start + 2.0 * inverse * t * control + t * t * end;
            let point = if x_axis {
                Point {
                    x: value,
                    y: f64::NAN,
                }
            } else {
                Point {
                    x: f64::NAN,
                    y: value,
                }
            };
            if let Some(current) = bounds {
                if x_axis {
                    current.0 = current.0.min(point.x);
                    current.2 = current.2.max(point.x);
                } else {
                    current.1 = current.1.min(point.y);
                    current.3 = current.3.max(point.y);
                }
            }
        }
    }
}

fn stroke_radius(size: f64, pressure: f64) -> f64 {
    size * sin((0.5 - 0.6 * (0.5 - pressure)) * std::f64::consts::PI / 2.0)
}

const fn point(x: f64, y: f64) -> Point {
    Point { x, y }
}

fn add(a: Point, b: Point) -> Point {
    point(a.x + b.x, a.y + b.y)
}

fn sub(a: Point, b: Point) -> Point {
    point(a.x - b.x, a.y - b.y)
}

fn mul(value: Point, scalar: f64) -> Point {
    point(value.x * scalar, value.y * scalar)
}

fn negate(value: Point) -> Point {
    point(-value.x, -value.y)
}

fn perpendicular(value: Point) -> Point {
    point(value.y, -value.x)
}

fn dot(a: Point, b: Point) -> f64 {
    a.x * b.x + a.y * b.y
}

fn equal(a: Point, b: Point) -> bool {
    a.x == b.x && a.y == b.y
}

fn unit(value: Point) -> Point {
    let length = hypot(value.x, value.y);
    point(value.x / length, value.y / length)
}

fn distance(a: Point, b: Point) -> f64 {
    hypot(a.x - b.x, a.y - b.y)
}

fn distance_squared(a: Point, b: Point) -> f64 {
    square(a.x - b.x) + square(a.y - b.y)
}

fn lerp(a: Point, b: Point, scalar: f64) -> Point {
    add(a, mul(sub(b, a), scalar))
}

fn rotate(value: Point, center: Point, radians: f64) -> Point {
    let (sine, cosine) = sin_cos(radians);
    let x = value.x - center.x;
    let y = value.y - center.y;
    point(
        x * cosine - y * sine + center.x,
        x * sine + y * cosine + center.y,
    )
}

fn project_point_distance(value: Point, direction: Point, distance: f64) -> Point {
    add(value, mul(direction, distance))
}
