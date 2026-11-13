//! The shapes of `pgflibraryshapes.callouts.code.tex`: ellipse, rectangle and cloud callouts.
//!
//! The pointer is relative (a vector from the shape's border, `callout relative pointer`) or
//! absolute (a point of the picture, `callout absolute pointer`); an absolute one is only known in
//! node coordinates once the node is placed, so the node computes the shape again with it.

use std::f64::consts::{FRAC_1_SQRT_2, SQRT_2};

use crate::scene::{Point, Segment};

use super::basic::ellipse_path;
use super::{angle_between, angle_between_lines, arc_to, border_ellipse, border_rectangle, polar, sin, symbols, Border, Geometry, Input};

/// Where the pointer goes, in node coordinates: the absolute pointer if there is one, otherwise
/// the relative pointer past the border point toward it (`border` gives that point for a
/// direction).
fn pointer(input: &mut Input, c: Point, border: impl Fn(Point) -> Point) -> Point {
    let absolute = !input.keys.text("/pgf/callout pointer@absolute").trim().is_empty();
    let p = match (absolute, input.pointer) {
        (true, Some(p)) => p,
        _ => {
            let rel = input.keys.point("/pgf/callout pointer@relative").unwrap_or(polar(300.0, 0.5));
            let b = border(rel);
            c.plus(b).plus(polar(angle_between(Point::new(0.0, 0.0), b), rel.norm()))
        }
    };
    // `callout pointer shorten`: toward the center
    let shorten = input.length("/pgf/callout pointer shorten", 0.0);
    p.plus(polar(angle_between(p, c), shorten))
}

/// The pointer anchor: the pointer moved out by the miter length of the outer sep.
fn pointer_anchor(input: &mut Input, p: Point, before: Point, after: Point) -> Point {
    let o = input.outer_sep();
    let aspect = input.number("/pgf/callout pointer anchor aspect", 1.0);
    let half = angle_between_lines(p, after, p, before) / 2.0;
    let dir = half + angle_between(p, after) + 180.0;
    p.plus(polar(dir, o / sin(half) * aspect))
}

pub fn ellipse_callout(input: &mut Input) -> Geometry {
    let (x, y) = input.half_with_inner();
    let xp = (SQRT_2 * x).max(input.minimum_width / 2.0);
    let yp = (SQRT_2 * y).max(input.minimum_height / 2.0);
    let (xr, yr) = (xp + input.outer_xsep, yp + input.outer_ysep);
    let arc = input.number("/pgf/callout pointer arc", 15.0);
    let c = input.center_point();
    let p = pointer(input, c, |d| border_ellipse(d, Point::new(xp, yp)));
    // the pointer leaves the ellipse around the point toward it, `arc` degrees wide
    let b = border_ellipse(p.minus(c), Point::new(xp, yp));
    let t = (b.y / yp).atan2(b.x / xp).to_degrees();
    let before_angle = (t - arc / 2.0).rem_euclid(360.0);
    let after_angle = (before_angle + arc).rem_euclid(360.0);
    let on = |a: f64| c.plus(Point::new(xp * a.to_radians().cos(), yp * a.to_radians().sin()));
    let (before, after) = (on(before_angle), on(after_angle));
    let mut g = Geometry::new(c, Border::Ellipse(Point::new(xr, yr)));
    let (m, bp) = (input.mid_point(), input.base_point());
    let across = |q: Point, sign: f64| {
        let dy = ((q.y - c.y) / yr).clamp(-1.0, 1.0);
        Point::new(c.x + sign * xr * (1.0 - dy * dy).sqrt(), q.y)
    };
    let k = FRAC_1_SQRT_2;
    let pa = pointer_anchor(input, p, before, after);
    for (name, q) in [
        ("center", c),
        ("mid", m),
        ("base", bp),
        ("mid east", across(m, 1.0)),
        ("mid west", across(m, -1.0)),
        ("base east", across(bp, 1.0)),
        ("base west", across(bp, -1.0)),
        ("north", c.plus(Point::new(0.0, yr))),
        ("south", c.plus(Point::new(0.0, -yr))),
        ("east", c.plus(Point::new(xr, 0.0))),
        ("west", c.plus(Point::new(-xr, 0.0))),
        ("north west", c.plus(Point::new(-k * xr, k * yr))),
        ("north east", c.plus(Point::new(k * xr, k * yr))),
        ("south west", c.plus(Point::new(-k * xr, -k * yr))),
        ("south east", c.plus(Point::new(k * xr, -k * yr))),
        ("pointer", pa),
    ] {
        g.anchor_at(name, q);
    }
    let mut v = vec![Segment::MoveTo(p), Segment::LineTo(after)];
    // pgf goes around in two arcs, up to 360° and on from 0°: their pieces start at 0°, so their
    // control points (which count for the picture's size) stay inside the ellipse's box
    if after_angle < before_angle {
        arc_to(&mut v, after, after_angle, before_angle, xp, yp);
    } else {
        let east = arc_to(&mut v, after, after_angle, 360.0, xp, yp);
        arc_to(&mut v, east, 0.0, before_angle, xp, yp);
    }
    v.push(Segment::Close);
    g.background(v);
    g
}

pub fn rectangle_callout(input: &mut Input) -> Geometry {
    let t = input.text();
    let xt = (input.inner_xsep + t.wd / 2.0).max(input.minimum_width / 2.0);
    let yt = (input.inner_ysep + (t.ht + t.dp) / 2.0).max(input.minimum_height / 2.0);
    let (xl, yl) = (xt + input.outer_xsep, yt + input.outer_ysep);
    let width = input.length("/pgf/callout pointer width", 0.25);
    let c = input.center_point();
    let p = pointer(input, c, |d| border_rectangle(d, Point::new(xt, yt)));
    // where the pointer joins the rectangle, kept away from the corners
    let b = border_rectangle(p.minus(c), Point::new(xt, yt));
    let angle = angle_between(Point::new(0.0, 0.0), b);
    // a box narrower than the pointer leaves it no room: its base stays at the middle of the side
    let (xa, ya) = ((xt - width).max(0.0), (yt - width).max(0.0));
    let (mut xb, mut yb) = (b.x, b.y);
    let (mut xc, mut yc) = (0.0, 0.0);
    let corner = |x: f64, y: f64| angle_between(Point::new(0.0, 0.0), Point::new(x, y));
    let pt = Point::new;
    let order = if angle < corner(xt, yt) {
        yc = width / 2.0;
        yb = yb.min(ya);
        [pt(xt, yt), pt(-xt, yt), pt(-xt, -yt), pt(xt, -yt)]
    } else if angle < corner(-xt, yt) {
        xc = -width / 2.0;
        xb = xb.clamp(-xa, xa);
        [pt(-xt, yt), pt(-xt, -yt), pt(xt, -yt), pt(xt, yt)]
    } else if angle < corner(-xt, -yt) {
        yc = -width / 2.0;
        yb = yb.clamp(-ya, ya);
        [pt(-xt, -yt), pt(xt, -yt), pt(xt, yt), pt(-xt, yt)]
    } else if angle < corner(xt, -yt) {
        xc = width / 2.0;
        xb = xb.clamp(-xa, xa);
        [pt(xt, -yt), pt(xt, yt), pt(-xt, yt), pt(-xt, -yt)]
    } else {
        yc = width / 2.0;
        yb = yb.max(-ya);
        [pt(xt, yt), pt(-xt, yt), pt(-xt, -yt), pt(xt, -yt)]
    };
    let before = c.plus(pt(xb - xc, yb - yc));
    let after = c.plus(pt(xb + xc, yb + yc));
    let pa = pointer_anchor(input, p, before, after);
    let mut g = Geometry::new(c, Border::RectangleShifted(pt(xl, yl), c));
    let bp = input.base_point();
    // pgf puts this mid point half an em up, not half an ex
    let m = pt(bp.x, 0.5 * input.ex / 0.43);
    for (name, q) in [
        ("center", c),
        ("mid", m),
        ("base", bp),
        ("mid east", m.plus(pt(xl, 0.0))),
        ("mid west", m.plus(pt(-xl, 0.0))),
        ("base east", bp.plus(pt(xl, 0.0))),
        ("base west", bp.plus(pt(-xl, 0.0))),
        ("north", c.plus(pt(0.0, yl))),
        ("south", c.plus(pt(0.0, -yl))),
        ("east", c.plus(pt(xl, 0.0))),
        ("west", c.plus(pt(-xl, 0.0))),
        ("north east", c.plus(pt(xl, yl))),
        ("south west", c.plus(pt(-xl, -yl))),
        ("south east", c.plus(pt(xl, -yl))),
        ("north west", c.plus(pt(-xl, yl))),
        ("pointer", pa),
    ] {
        g.anchor_at(name, q);
    }
    let mut v = vec![Segment::MoveTo(before), Segment::LineTo(p), Segment::LineTo(after)];
    v.extend(order.iter().map(|q| Segment::LineTo(c.plus(*q))));
    v.push(Segment::Close);
    g.background(v);
    g
}

/// `.2 of callout`, `2pt and 3pt`, `2pt`: a size of the cloud callout's pointer puffs, from the
/// size `w` × `h` of the cloud.
fn pointer_size(input: &mut Input, key: &str, w: f64, h: f64) -> (f64, f64) {
    let text = input.keys.text(key);
    if let Some((k, _)) = text.split_once("of callout") {
        let k: f64 = k.trim().parse().unwrap_or(0.0);
        return (k * w, k * h);
    }
    // pgf takes the second size for both
    let size = match text.split_once("and") {
        Some((_, b)) => b.trim().to_string(),
        None => text.trim().to_string(),
    };
    let v = input.keys.length_of(&size, 0.0);
    (v, v)
}

pub fn cloud_callout(input: &mut Input) -> Geometry {
    let mut g = symbols::cloud(input);
    let cloud = symbols::cloud_parts(input);
    let c = input.center_point();
    let outer = cloud.outer;
    let p = pointer(input, c, |d| border_ellipse(d, outer));
    g.anchor_at("pointer", p);
    let b = c.plus(border_ellipse(p.minus(c), outer));
    let angle = angle_between(p, c);
    let (sx, sy) = pointer_size(input, "/pgf/callout pointer start size", 2.0 * outer.x, 2.0 * cloud.inner.y);
    let (ex, ey) = pointer_size(input, "/pgf/callout pointer end size", 2.0 * outer.x, 2.0 * cloud.inner.y);
    let n = input.number("/pgf/callout pointer segments", 2.0).trunc().max(1.0) as usize;
    let (dx, dy) = ((sx - ex) / n as f64, (sy - ey) / n as f64);
    let step = b.minus(p).norm() / n as f64;
    let (mut rx, mut ry) = (ex, ey);
    for k in 0..n {
        let q = p.plus(polar(angle, step * k as f64));
        let e = ellipse_path(q, rx / 2.0, ry / 2.0);
        if let Some(bg) = g.paths.iter_mut().find(|s| s.layer == super::Layer::Background) {
            bg.segments.extend(e);
        }
        rx += dx;
        ry += dy;
    }
    g
}
