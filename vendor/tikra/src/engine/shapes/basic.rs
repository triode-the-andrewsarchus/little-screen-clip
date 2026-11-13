//! The shapes of `pgfmoduleshapes.code.tex` (coordinate, rectangle, circle) and the ellipse and
//! diamond of `shapes.geometric`.

use std::f64::consts::{FRAC_1_SQRT_2, SQRT_2};

use crate::scene::{Point, Segment};

use super::{Border, Geometry, Input};

/// `\pgfpathellipse`: a closed ellipse of radii `rx`, `ry` around `c`, from its east point,
/// counterclockwise, in four curves.
pub fn ellipse_path(c: Point, rx: f64, ry: f64) -> Vec<Segment> {
    let k = 0.55228475;
    let p = |x: f64, y: f64| c.plus(Point::new(x, y));
    vec![
        Segment::MoveTo(p(rx, 0.0)),
        Segment::CurveTo(p(rx, k * ry), p(k * rx, ry), p(0.0, ry)),
        Segment::CurveTo(p(-k * rx, ry), p(-rx, k * ry), p(-rx, 0.0)),
        Segment::CurveTo(p(-rx, -k * ry), p(-k * rx, -ry), p(0.0, -ry)),
        Segment::CurveTo(p(k * rx, -ry), p(rx, -k * ry), p(rx, 0.0)),
        Segment::Close,
    ]
}

/// A shape with nothing drawn and every anchor at its center.
pub fn coordinate(input: &mut Input) -> Geometry {
    let c = input.center_point();
    let mut g = Geometry::new(c, Border::Point);
    for a in ["center", "north", "north west", "north east", "west", "east", "mid", "mid west", "mid east", "base", "base west", "base east", "south", "south west", "south east"] {
        g.anchor_at(a, c);
    }
    // no text
    g.parts.clear();
    g
}

/// The anchors of a box from `sw` to `ne`, as the rectangle has them (and the shapes that
/// inherit them).
pub fn box_anchors(g: &mut Geometry, sw: Point, ne: Point, ex: f64) {
    let c = sw.lerp(ne, 0.5);
    let m = 0.5 * ex;
    for (name, p) in [
        ("center", c),
        ("mid", Point::new(c.x, m)),
        ("base", Point::new(c.x, 0.0)),
        ("north", Point::new(c.x, ne.y)),
        ("south", Point::new(c.x, sw.y)),
        ("west", Point::new(sw.x, c.y)),
        ("mid west", Point::new(sw.x, m)),
        ("base west", Point::new(sw.x, 0.0)),
        ("north west", Point::new(sw.x, ne.y)),
        ("south west", sw),
        ("east", Point::new(ne.x, c.y)),
        ("mid east", Point::new(ne.x, m)),
        ("base east", Point::new(ne.x, 0.0)),
        ("north east", ne),
        ("south east", Point::new(ne.x, sw.y)),
    ] {
        g.anchor_at(name, p);
    }
}

/// The text box with inner sep, enlarged to the minimum size: its south west and north east
/// corners, with outer sep (`\southwest`, `\northeast` of the rectangle).
pub fn rectangle_corners(input: &Input) -> (Point, Point) {
    let t = input.text();
    let w = (t.wd + 2.0 * input.inner_xsep).max(input.minimum_width);
    let h = (t.ht + t.dp + 2.0 * input.inner_ysep).max(input.minimum_height);
    let cy = (t.ht - t.dp) / 2.0;
    let ne = Point::new(t.wd / 2.0 + w / 2.0 + input.outer_xsep, cy + h / 2.0 + input.outer_ysep);
    let sw = Point::new(t.wd / 2.0 - w / 2.0 - input.outer_xsep, cy - h / 2.0 - input.outer_ysep);
    (sw, ne)
}

pub fn rectangle(input: &mut Input) -> Geometry {
    let (sw, ne) = rectangle_corners(input);
    let c = sw.lerp(ne, 0.5);
    let mut g = Geometry::new(c, Border::Rectangle(ne.minus(sw).scale(0.5)));
    box_anchors(&mut g, sw, ne, input.ex);
    let o = Point::new(input.outer_xsep, input.outer_ysep);
    let (a, b) = (sw.plus(o), ne.minus(o));
    // \pgfpathrectanglecorners, as the "re" operator draws it
    g.background(vec![Segment::MoveTo(a), Segment::LineTo(Point::new(b.x, a.y)), Segment::LineTo(b), Segment::LineTo(Point::new(a.x, b.y)), Segment::Close]);
    g.roundable = true;
    g
}

/// The anchors of a round shape of radii `r` around `c`: the corners at 45° of the radii.
fn round_anchors(g: &mut Geometry, c: Point, r: Point, ex: f64) {
    let m = 0.5 * ex;
    let k = FRAC_1_SQRT_2;
    for (name, p) in [
        ("center", c),
        ("mid", Point::new(c.x, m)),
        ("base", Point::new(c.x, 0.0)),
        ("north", Point::new(c.x, c.y + r.y)),
        ("south", Point::new(c.x, c.y - r.y)),
        ("west", Point::new(c.x - r.x, c.y)),
        ("east", Point::new(c.x + r.x, c.y)),
        ("mid west", Point::new(c.x - r.x, m)),
        ("mid east", Point::new(c.x + r.x, m)),
        ("base west", Point::new(c.x - r.x, 0.0)),
        ("base east", Point::new(c.x + r.x, 0.0)),
        ("north west", Point::new(c.x - k * r.x, c.y + k * r.y)),
        ("south west", Point::new(c.x - k * r.x, c.y - k * r.y)),
        ("north east", Point::new(c.x + k * r.x, c.y + k * r.y)),
        ("south east", Point::new(c.x + k * r.x, c.y - k * r.y)),
    ] {
        g.anchor_at(name, p);
    }
}

pub fn circle(input: &mut Input) -> Geometry {
    let c = input.center_point();
    let (xa, ya) = input.half_with_inner();
    let r = xa.hypot(ya).max(input.minimum_width / 2.0).max(input.minimum_height / 2.0) + input.outer_sep();
    let mut g = Geometry::new(c, Border::Ellipse(Point::new(r, r)));
    round_anchors(&mut g, c, Point::new(r, r), input.ex);
    let rr = r - input.outer_sep();
    g.background(ellipse_path(c, rr, rr));
    g
}

pub fn ellipse(input: &mut Input) -> Geometry {
    let c = input.center_point();
    let (xa, ya) = input.half_with_inner();
    let rx = (SQRT_2 * xa).max(input.minimum_width / 2.0) + input.outer_xsep;
    let ry = (SQRT_2 * ya).max(input.minimum_height / 2.0) + input.outer_ysep;
    let mut g = Geometry::new(c, Border::Ellipse(Point::new(rx, ry)));
    round_anchors(&mut g, c, Point::new(rx, ry), input.ex);
    g.background(ellipse_path(c, rx - input.outer_xsep, ry - input.outer_ysep));
    g
}

pub fn diamond(input: &mut Input) -> Geometry {
    let c = input.center_point();
    let (xa, ya) = input.half_with_inner();
    let aspect = if input.aspect.abs() > 1e-9 { input.aspect } else { 1.0 };
    let x = (xa + aspect * ya).max(input.minimum_width / 2.0) + input.outer_xsep;
    let y = (xa / aspect + ya).max(input.minimum_height / 2.0) + input.outer_ysep;
    let p = |dx: f64, dy: f64| c.plus(Point::new(dx, dy));
    let outline = vec![p(x, 0.0), p(0.0, y), p(-x, 0.0), p(0.0, -y)];
    let mut g = Geometry::new(c, Border::Outline(outline));
    for (name, q) in [
        ("center", c),
        ("mid", Point::new(c.x, 0.5 * input.ex)),
        ("base", Point::new(c.x, 0.0)),
        ("north", p(0.0, y)),
        ("south", p(0.0, -y)),
        ("west", p(-x, 0.0)),
        ("east", p(x, 0.0)),
        ("north west", p(-x / 2.0, y / 2.0)),
        ("south west", p(-x / 2.0, -y / 2.0)),
        ("north east", p(x / 2.0, y / 2.0)),
        ("south east", p(x / 2.0, -y / 2.0)),
    ] {
        g.anchor_at(name, q);
    }
    // the drawn diamond is in by the outer sep times √2 (not by the miter length)
    let xc = x - SQRT_2 * input.outer_xsep;
    let yc = y - SQRT_2 * input.outer_ysep;
    g.background(vec![Segment::MoveTo(p(xc, 0.0)), Segment::LineTo(p(0.0, yc)), Segment::LineTo(p(-xc, 0.0)), Segment::LineTo(p(0.0, -yc)), Segment::Close]);
    g.roundable = true;
    g
}
