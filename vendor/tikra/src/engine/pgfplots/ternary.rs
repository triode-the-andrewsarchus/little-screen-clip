//! Ternary axes (`tikzlibrarypgfplots.ternary.code.tex`): three axes that sum to one, drawn as
//! an equilateral triangle of side `width`. A point's parts (each mapped to [0, 1] by its axis'
//! limits) place it at `0.5·(x + 2z)` across and `√3/2·x` up: y = 1 is the lower left corner, z = 1
//! the lower right one and x = 1 the top. Each axis' ticks are on one edge (x on the right, y on
//! the left, z at the bottom), grid lines run parallel to the next edge, the tick lines continue
//! them outwards and the labels sit at their ends. With `ternary limits relative` (the default)
//! the tick labels are percentages. Plain coordinates inside the axis are cartesian, in units of
//! the side (`cartesian cs`).

use crate::scene::{Point, Segment};
use crate::Error;

use super::draw::{axis_char, minor_ticks, tick_labels_at, tick_offset, unit, visible, Frame, Setup};
use super::keys;
use super::Engine;

/// √3/2, as pgfplots writes it.
const H: f64 = 0.866025403784;

/// The axis vectors (canvas) for a side of `w`: x from z = 1 to x = 1, y from x = 1 to y = 1, z
/// from y = 1 to z = 1 (`\pgfplotspoint@initialisation@axes@ternary`).
pub fn vectors(w: f64) -> [Point; 3] {
    [Point::new(-0.5 * w, H * w), Point::new(-0.5 * w, -H * w), Point::new(w, 0.0)]
}

/// The axis vectors of a frame (its `e` are them per logical unit).
fn sides(f: &Frame) -> [Point; 3] {
    [0, 1, 2].map(|i| f.e[i].scale(f.max[i] - f.min[i]))
}

/// The canvas point of parts in [0, 1].
pub fn at_unit(f: &Frame, u: [f64; 3]) -> Point {
    let v = sides(f);
    // y = 1 is the origin: x adds -y's vector, z adds z's
    f.origin.plus(v[1].scale(-u[0])).plus(v[2].scale(u[2]))
}

/// The part of logical value `v` on axis `a`.
pub fn part(f: &Frame, a: usize, v: f64) -> f64 {
    (v - f.min[a]) / (f.max[a] - f.min[a])
}

/// The canvas point of logical coordinates; a missing z is what x and y leave of one.
pub fn at(f: &Frame, l: [f64; 3]) -> Point {
    let mut u = [0, 1, 2].map(|a| part(f, a, l[a]));
    if !u[2].is_finite() {
        u[2] = 1.0 - u[0] - u[1];
    }
    at_unit(f, u)
}

/// Plain coordinates: cartesian, in units of the side (`\pgfplotspointcartesian@`).
pub fn cartesian(f: &Frame, x: f64, y: f64) -> Point {
    let w = sides(f)[2].x;
    f.origin.plus(Point::new(x * w, y * w))
}

/// The triangle's corners: y = 1, z = 1, x = 1.
pub fn corners(f: &Frame) -> [Point; 3] {
    [at_unit(f, [0.0, 1.0, 0.0]), at_unit(f, [0.0, 0.0, 1.0]), at_unit(f, [1.0, 0.0, 0.0])]
}

/// The axis that the grid lines of axis `a` run along, and the one held at zero on its edge.
fn b_axis(a: usize) -> usize {
    [2, 0, 1][a]
}

/// The point at part `t` of axis `a` on its edge (the other axis there is at zero).
pub fn edge(f: &Frame, a: usize, t: f64) -> Point {
    let mut u = [0.0; 3];
    u[a] = t;
    u[b_axis(a)] = 1.0 - t;
    at_unit(f, u)
}

/// The far end of the grid line from part `t` on axis `a`'s edge.
fn far(f: &Frame, a: usize, t: f64) -> Point {
    let mut u = [0.0; 3];
    u[a] = t;
    u[3 - a - b_axis(a)] = 1.0 - t;
    at_unit(f, u)
}

/// The outward normal of axis `a`'s edge (`\pgfplotspointouternormalvectorofaxis@ternary`).
pub fn normal(f: &Frame, a: usize) -> Point {
    let v = sides(f);
    let n = match a {
        0 => v[2].minus(v[1]),
        1 => v[0].minus(v[2]),
        _ => v[1].minus(v[0]),
    };
    unit(n)
}

/// The direction ticks of axis `a` point out of the triangle, along its grid lines.
fn outward(f: &Frame, a: usize) -> Point {
    unit(sides(f)[b_axis(a)])
}

/// The triangle as a closed path (the clip path and the axis lines).
pub fn outline(f: &Frame) -> Vec<Segment> {
    let [y, z, x] = corners(f);
    vec![Segment::MoveTo(z), Segment::LineTo(x), Segment::LineTo(y), Segment::Close]
}

/// A tick label's value as shown: a percentage of the range with relative limits.
pub fn shown(f: &Frame, a: usize, t: f64) -> f64 {
    if f.ternary_relative {
        100.0 * part(f, a, t)
    } else {
        t
    }
}

/// The ternary axis: grid lines, ticks and tick labels of each axis, then the triangle.
pub(super) fn draw_axis(e: &mut Engine, s: &Setup) -> Result<(), Error> {
    let f = s.frame.clone();
    // pgfplots draws the edges for y, z, x in this order
    for a in [1usize, 2, 0] {
        if s.hide[a] {
            continue;
        }
        let c = axis_char(a);
        let list = &s.ticks[a];
        let major: Vec<f64> = visible(&f, a, list).into_iter().map(|(_, t)| t).collect();
        let minor = minor_ticks(e, s, a, list);
        let line = |t: f64| {
            let u = part(&f, a, t);
            vec![Segment::MoveTo(edge(&f, a, u)), Segment::LineTo(far(&f, a, u))]
        };
        if keys::flag(e, &format!("/pgfplots/{c}minorgrids")) {
            let segs: Vec<Segment> = minor.iter().flat_map(|t| line(*t)).collect();
            e.draw_with(&format!("/pgfplots/every axis grid,/pgfplots/every minor grid,/pgfplots/every axis {c} grid,/pgfplots/every minor {c} grid"), segs)?;
        }
        if keys::flag(e, &format!("/pgfplots/{c}majorgrids")) {
            let segs: Vec<Segment> = major.iter().flat_map(|t| line(*t)).collect();
            e.draw_with(&format!("/pgfplots/every axis grid,/pgfplots/every major grid,/pgfplots/every axis {c} grid,/pgfplots/every major {c} grid"), segs)?;
        }
        // tick lines along the grid lines' direction, outwards
        let d = outward(&f, a);
        let align = e.pv(&format!("{c}tick align"));
        let mut kinds = Vec::new();
        if keys::flag(e, &format!("/pgfplots/{c}minorticks")) && !minor.is_empty() {
            kinds.push(("minor", minor.clone(), keys::length(e, "/pgfplots/minor tick length").unwrap_or(0.1)));
        }
        if keys::flag(e, &format!("/pgfplots/{c}majorticks")) {
            kinds.push(("major", major.clone(), keys::length(e, "/pgfplots/major tick length").unwrap_or(0.15)));
        }
        for (kind, ticks, len) in kinds {
            let (from, to) = match align.as_str() {
                "outside" => (0.0, len),
                "center" => (-len / 2.0, len / 2.0),
                _ => (-len, 0.0),
            };
            let segs: Vec<Segment> = ticks
                .iter()
                .flat_map(|t| {
                    let p = edge(&f, a, part(&f, a, *t));
                    [Segment::MoveTo(p.plus(d.scale(from))), Segment::LineTo(p.plus(d.scale(to)))]
                })
                .collect();
            e.draw_with(&format!("/pgfplots/every tick,/pgfplots/every {kind} tick,/pgfplots/every {c} tick,/pgfplots/every {kind} {c} tick"), segs)?;
        }
        // tick labels at the ticks' outer ends
        if keys::flag(e, &format!("/pgfplots/{c}majorticks")) {
            let off = tick_offset(e, a);
            let g = f.clone();
            tick_labels_at(e, s, a, list, false, &|t| edge(&g, a, part(&g, a, t)), d.scale(off), off)?;
        }
    }
    if s.hide.iter().all(|h| !h) {
        e.draw_with("/pgfplots/every outer x axis line,/pgfplots/every outer y axis line", outline(&f))?;
    } else {
        let [y, z, x] = corners(&f);
        for (a, p, q) in [(0usize, z, x), (1, x, y), (2, y, z)] {
            if !s.hide[a] {
                e.draw_with(&format!("/pgfplots/every outer {} axis line", axis_char(a)), vec![Segment::MoveTo(p), Segment::LineTo(q)])?;
            }
        }
    }
    Ok(())
}
