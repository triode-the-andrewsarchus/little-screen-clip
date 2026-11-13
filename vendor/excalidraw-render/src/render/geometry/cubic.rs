//! Generic bounds of one cubic. Path traversal and union policy live in the parent.

use super::Point;
use crate::portable_math::{cube, square};

/// Endpoint and extrema bounds, in `(min_x, min_y, max_x, max_y)` order.
/// Keep the arithmetic order and root tests: these bounds feed canvas pivots
/// and coverage accounting, independently of the reference scene bounds.
pub(super) fn bounds(
    start: Point,
    control_1: Point,
    control_2: Point,
    end: Point,
) -> (f64, f64, f64, f64) {
    let mut values_x = [start.x, end.x, 0.0, 0.0];
    let mut values_y = [start.y, end.y, 0.0, 0.0];
    let count_x = 2 + cubic_extrema(start.x, control_1.x, control_2.x, end.x, &mut values_x[2..]);
    let count_y = 2 + cubic_extrema(start.y, control_1.y, control_2.y, end.y, &mut values_y[2..]);
    let min_x = values_x[..count_x]
        .iter()
        .copied()
        .fold(f64::INFINITY, f64::min);
    let min_y = values_y[..count_y]
        .iter()
        .copied()
        .fold(f64::INFINITY, f64::min);
    let max_x = values_x[..count_x]
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let max_y = values_y[..count_y]
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    (min_x, min_y, max_x, max_y)
}

fn cubic_extrema(
    start: f64,
    control_1: f64,
    control_2: f64,
    end: f64,
    output: &mut [f64],
) -> usize {
    let a = -start + 3.0 * control_1 - 3.0 * control_2 + end;
    let b = 2.0 * (start - 2.0 * control_1 + control_2);
    let c = control_1 - start;
    let mut count = 0;
    if a.abs() <= f64::EPSILON {
        if b != 0.0 {
            count += retain_cubic_extremum(
                -c / b,
                start,
                control_1,
                control_2,
                end,
                &mut output[count..],
            );
        }
        return count;
    }
    let discriminant = b * b - 4.0 * a * c;
    if discriminant < 0.0 {
        return 0;
    }
    let root = discriminant.sqrt();
    count += retain_cubic_extremum(
        (-b + root) / (2.0 * a),
        start,
        control_1,
        control_2,
        end,
        &mut output[count..],
    );
    count += retain_cubic_extremum(
        (-b - root) / (2.0 * a),
        start,
        control_1,
        control_2,
        end,
        &mut output[count..],
    );
    count
}

fn retain_cubic_extremum(
    t: f64,
    start: f64,
    control_1: f64,
    control_2: f64,
    end: f64,
    output: &mut [f64],
) -> usize {
    if !(0.0..=1.0).contains(&t) || output.is_empty() {
        return 0;
    }
    let inverse = 1.0 - t;
    output[0] = cube(inverse) * start
        + 3.0 * square(inverse) * t * control_1
        + 3.0 * inverse * square(t) * control_2
        + cube(t) * end;
    1
}
