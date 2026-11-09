use super::parser::value;
use std::f64::consts::TAU;
pub(super) fn points(
    start: [f64; 3],
    end: [f64; 3],
    words: &[(u8, f64)],
    unit: f64,
    clockwise: bool,
) -> Result<Vec<[f64; 3]>, String> {
    let center = if let Some(radius) = value(words, b'R') {
        if value(words, b'I').is_some() || value(words, b'J').is_some() {
            return Err("arc cannot combine R and I/J".into());
        }
        radius_center(start, end, radius * unit, clockwise)?
    } else {
        if value(words, b'I').is_none() && value(words, b'J').is_none() {
            return Err("arc requires I/J offsets or R radius".into());
        }
        [
            start[0] + value(words, b'I').unwrap_or(0.0) * unit,
            start[1] + value(words, b'J').unwrap_or(0.0) * unit,
        ]
    };
    let radius = (start[0] - center[0]).hypot(start[1] - center[1]);
    if center.iter().any(|v| v.abs() > 1e9) || radius > 1e9 {
        return Err("arc exceeds coordinate preview limits".into());
    }
    let other = (end[0] - center[0]).hypot(end[1] - center[1]);
    if radius < 1e-9 || (radius - other).abs() > 0.01_f64.max(radius * 0.0001) {
        return Err("arc start/end radii disagree".into());
    }
    let first = (start[1] - center[1]).atan2(start[0] - center[0]);
    let last = (end[1] - center[1]).atan2(end[0] - center[0]);
    let angle = sweep(first, last, clockwise);
    let count = (angle.abs() / (TAU / 180.0)).ceil().clamp(1.0, 180.0) as usize;
    let mut points = Vec::with_capacity(count);
    for index in 1..=count {
        let t = index as f64 / count as f64;
        let a = first + angle * t;
        points.push(if index == count {
            end
        } else {
            [
                center[0] + radius * a.cos(),
                center[1] + radius * a.sin(),
                start[2] + (end[2] - start[2]) * t,
            ]
        });
    }
    Ok(points)
}
fn sweep(first: f64, last: f64, clockwise: bool) -> f64 {
    let angle = if clockwise {
        (first - last).rem_euclid(TAU)
    } else {
        (last - first).rem_euclid(TAU)
    };
    let angle = if angle < 1e-12 { TAU } else { angle };
    if clockwise { -angle } else { angle }
}
fn radius_center(
    start: [f64; 3],
    end: [f64; 3],
    radius: f64,
    clockwise: bool,
) -> Result<[f64; 2], String> {
    let dx = end[0] - start[0];
    let dy = end[1] - start[1];
    let chord = dx.hypot(dy);
    if chord < 1e-9 || chord > 2.0 * radius.abs() {
        return Err("R arc has impossible radius or coincident endpoints".into());
    }
    let height = (radius * radius - chord * chord / 4.0).max(0.0).sqrt();
    let sign = if clockwise ^ (radius < 0.0) {
        -1.0
    } else {
        1.0
    };
    Ok([
        (start[0] + end[0]) / 2.0 - sign * dy * height / chord,
        (start[1] + end[1]) / 2.0 + sign * dx * height / chord,
    ])
}
