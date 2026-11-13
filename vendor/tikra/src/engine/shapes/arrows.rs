//! The shapes of `pgflibraryshapes.arrows.code.tex`: single arrow, double arrow and arrow box.

use std::f64::consts::SQRT_2;

use crate::scene::Point;

use super::{cot, miter_offset, polygon_path, ray, rotate_around, Border, Geometry, Input};

/// The eight compass anchors as border points from the center.
fn compass(g: &mut Geometry) {
    for (name, d) in [
        ("north", (0.0, 1.0)),
        ("south", (0.0, -1.0)),
        ("east", (1.0, 0.0)),
        ("west", (-1.0, 0.0)),
        ("north east", (1.0, 1.0)),
        ("north west", (-1.0, 1.0)),
        ("south east", (1.0, -1.0)),
        ("south west", (-1.0, -1.0)),
    ] {
        let p = g.border_point(Point::new(d.0, d.1));
        g.anchor_at(name, p);
    }
}

fn text_and_side_anchors(g: &mut Geometry, input: &Input, outline: &[Point]) {
    let (m, b) = (input.mid_point(), input.base_point());
    for (name, p) in [
        ("center", input.center_point()),
        ("mid", m),
        ("base", b),
        ("mid west", ray(m, Point::new(-1.0, 0.0), outline)),
        ("mid east", ray(m, Point::new(1.0, 0.0), outline)),
        ("base west", ray(b, Point::new(-1.0, 0.0), outline)),
        ("base east", ray(b, Point::new(1.0, 0.0), outline)),
    ] {
        g.anchor_at(name, p);
    }
}

/// What the single and double arrows share: the half sizes of the body (line width included, as
/// pgf has it), the rotation and the outer sep along the arrow.
struct Body {
    xa: f64,
    ya: f64,
    rotate: f64,
    outer: f64,
}

fn body(input: &mut Input) -> Body {
    let (x, y) = input.half_with_inner();
    let (mut xa, mut ya) = (x + input.line_width / 2.0, y + input.line_width / 2.0);
    let (rotate, incircle) = input.border_rotation();
    let mut outer = input.outer_xsep;
    if incircle {
        xa = SQRT_2 * xa.max(ya);
        ya = xa;
        outer = input.outer_sep();
    } else if rotate == 90.0 || rotate == 270.0 {
        (xa, ya) = (ya, xa);
        outer = input.outer_ysep;
    }
    Body { xa, ya, rotate, outer }
}

/// The head of an arrow: `(xb, xc, yc, ya)` as pgf names them, with the minimum width applied.
fn head(input: &Input, half: f64, extend: f64, mut ya: f64) -> (f64, f64, f64, f64) {
    let mut xb = cot(half) * ya;
    let mut yc = extend;
    let mut xc = extend + ya;
    if xc < input.minimum_width / 2.0 {
        let s = (input.minimum_width / 2.0) / xc;
        ya *= s;
        xc *= s;
        yc *= s;
        xb *= s;
    }
    let xc = cot(half) * (xc - ya);
    (xb, xc, yc, ya)
}

pub fn single_arrow(input: &mut Input) -> Geometry {
    let Body { mut xa, ya, rotate, outer } = body(input);
    let half = input.number("/pgf/single arrow tip angle", 90.0) / 2.0;
    let extend = input.length("/pgf/single arrow head extend", 0.25);
    let indent = input.length("/pgf/single arrow head indent", 0.0);
    let (xb, xc, yc, ya) = head(input, half, extend, ya);
    xa = (2.0 * xa + xb).max(input.minimum_height);
    xa = (xa - xb) / 2.0;
    let c = input.center_point();
    let p = |x: f64, y: f64| c.plus(Point::new(x, y));
    let shape = [p(xa + xb, 0.0), p(xa - xc, ya + yc), p(xa - xc + indent, ya), p(-xa, ya), p(-xa, -ya), p(xa - xc + indent, -ya), p(xa - xc, -ya - yc)];
    let rot = |v: Vec<Point>| -> Vec<Point> { v.into_iter().map(|q| rotate_around(q, c, rotate)).collect() };
    let b = rot(miter_offset(&shape, outer));
    let mut g = Geometry::new(c, Border::Outline(b.clone()));
    text_and_side_anchors(&mut g, input, &b);
    compass(&mut g);
    for (name, k) in [("tip", 0), ("before tip", 1), ("before head", 2), ("after tail", 3), ("before tail", 4), ("after head", 5), ("after tip", 6)] {
        g.anchor_at(name, b[k]);
    }
    g.anchor_at("tail", b[3].lerp(b[4], 0.5));
    g.background(polygon_path(&rot(shape.to_vec())));
    g.roundable = true;
    g
}

pub fn double_arrow(input: &mut Input) -> Geometry {
    let Body { mut xa, ya, rotate, outer } = body(input);
    let half = input.number("/pgf/double arrow tip angle", 90.0) / 2.0;
    let extend = input.length("/pgf/double arrow head extend", 0.25);
    let indent = input.length("/pgf/double arrow head indent", 0.0);
    let (xb, xc, yc, ya) = head(input, half, extend, ya);
    xa = (2.0 * (xa + xb)).max(input.minimum_height) / 2.0 - xb;
    let c = input.center_point();
    let p = |x: f64, y: f64| c.plus(Point::new(x, y));
    let (tip, bt, bh) = (Point::new(xa + xb, 0.0), Point::new(xa - xc, ya + yc), Point::new(xa - xc + indent, ya));
    let shape = [p(tip.x, 0.0), p(bt.x, bt.y), p(bh.x, bh.y), p(-bh.x, bh.y), p(-bt.x, bt.y), p(-tip.x, 0.0), p(-bt.x, -bt.y), p(-bh.x, -bh.y), p(bh.x, -bh.y), p(bt.x, -bt.y)];
    let rot = |v: Vec<Point>| -> Vec<Point> { v.into_iter().map(|q| rotate_around(q, c, rotate)).collect() };
    let b = rot(miter_offset(&shape, outer));
    let mut g = Geometry::new(c, Border::Outline(b.clone()));
    text_and_side_anchors(&mut g, input, &b);
    compass(&mut g);
    for (name, k) in [
        ("tip 1", 0),
        ("before tip 1", 1),
        ("before head 1", 2),
        ("after head 2", 3),
        ("after tip 2", 4),
        ("tip 2", 5),
        ("before tip 2", 6),
        ("before head 2", 7),
        ("after head 1", 8),
        ("after tip 1", 9),
    ] {
        g.anchor_at(name, b[k]);
    }
    g.background(polygon_path(&rot(shape.to_vec())));
    g.roundable = true;
    g
}

/// An arrow's length from the center: `none` is 0, a length is from the box's side unless it says
/// `from center`.
fn arrow_extend(input: &mut Input, side: &str, half_box: f64) -> f64 {
    let key = format!("/pgf/arrow box {side} arrow");
    let mut text = input.keys.text(&key);
    // `arrow box arrows={east:1cm, north}`: the listed arrows (a direction alone takes the
    // length before it), the others none
    let list = input.keys.text("/pgf/arrow box arrow list");
    if !list.trim().is_empty() {
        let mut length = "0pt".to_string();
        text = "0pt".into();
        for item in list.split(',') {
            let (dir, l) = match item.split_once(':') {
                Some((d, l)) => (d.trim(), Some(l.trim())),
                None => (item.trim(), None),
            };
            if let Some(l) = l {
                length = l.to_string();
            }
            if dir == side {
                text = length.clone();
            }
        }
    }
    if text.contains("none") {
        return 0.0;
    }
    let length_text = text.replace("from center", "").replace("from border", "");
    let v = input.keys.length_of(length_text.trim(), 0.0);
    if !text.contains("from center") && v > 0.0 {
        v + half_box
    } else {
        v
    }
}

pub fn arrow_box(input: &mut Input) -> Geometry {
    let t = input.text();
    let (ox, oy) = (input.outer_xsep, input.outer_ysep);
    let w = (input.inner_xsep + t.wd / 2.0).max(input.minimum_width / 2.0) + ox;
    let h = (input.inner_ysep + (t.ht + t.dp) / 2.0).max(input.minimum_height / 2.0) + oy;
    let west = arrow_extend(input, "west", w);
    let east = arrow_extend(input, "east", w);
    let north = arrow_extend(input, "north", h);
    let south = arrow_extend(input, "south", h);
    let s = input.length("/pgf/arrow box shaft width", 0.125) / 2.0;
    let he = input.length("/pgf/arrow box head extend", 0.125);
    let hi = input.length("/pgf/arrow box head indent", 0.0);
    let half = input.number("/pgf/arrow box tip angle", 90.0) / 2.0;
    // the point just before an arrow tip, for the east arrow
    let bt = Point::new(cot(half) * (s + he), s + he);
    let c = input.center_point();
    let p = |x: f64, y: f64| c.plus(Point::new(x, y));
    let (cx, cy) = (w - ox, h - oy);
    let mut shape = vec![p(cx, cy)];
    // each arrow as seven points, in the path's order, with the names of their anchors
    let mut named: Vec<(String, usize)> = Vec::new();
    let mut arrow = |shape: &mut Vec<Point>, side: &str, pts: [Point; 7]| {
        for (k, q) in pts.iter().enumerate() {
            let name = match k {
                0 => format!("before {side} arrow"),
                1 => format!("before {side} arrow head"),
                2 => format!("before {side} arrow tip"),
                3 => format!("{side} arrow tip"),
                4 => format!("after {side} arrow tip"),
                5 => format!("after {side} arrow head"),
                _ => format!("after {side} arrow"),
            };
            named.push((name, shape.len()));
            shape.push(c.plus(*q));
        }
    };
    let pt = Point::new;
    if east > 0.0 {
        let x = -bt.x + east;
        arrow(&mut shape, "east", [pt(cx, s), pt(x + hi, s), pt(x, bt.y), pt(east, 0.0), pt(x, -bt.y), pt(x + hi, -s), pt(cx, -s)]);
    }
    shape.push(p(cx, -cy));
    if south > 0.0 {
        let y = bt.x - south;
        arrow(&mut shape, "south", [pt(s, -cy), pt(s, y - hi), pt(bt.y, y), pt(0.0, -south), pt(-bt.y, y), pt(-s, y - hi), pt(-s, -cy)]);
    }
    shape.push(p(-cx, -cy));
    if west > 0.0 {
        let x = bt.x - west;
        arrow(&mut shape, "west", [pt(-cx, -s), pt(x - hi, -s), pt(x, -bt.y), pt(-west, 0.0), pt(x, bt.y), pt(x - hi, s), pt(-cx, s)]);
    }
    shape.push(p(-cx, cy));
    if north > 0.0 {
        let y = -bt.x + north;
        arrow(&mut shape, "north", [pt(-s, cy), pt(-s, y + hi), pt(-bt.y, y), pt(0.0, north), pt(bt.y, y), pt(s, y + hi), pt(s, cy)]);
    }
    let b = miter_offset(&shape, input.outer_sep());
    let mut g = Geometry::new(c, Border::Outline(b.clone()));
    for (name, k) in &named {
        g.anchor_at(name, b[*k]);
    }
    let (m, bp) = (input.mid_point(), input.base_point());
    let side = |o: Point, ext: f64, d: f64| if ext > 0.0 { ray(o, Point::new(d, 0.0), &b) } else { o.plus(Point::new(d * w, 0.0)) };
    let tip = |ext: f64, name: &str, fallback: Point, g: &Geometry| if ext > 0.0 { g.anchor(&format!("{name} arrow tip")).unwrap_or(fallback) } else { fallback };
    let anchors = [
        ("center", c),
        ("mid", m),
        ("base", bp),
        ("mid east", side(m, east, 1.0)),
        ("mid west", side(m, west, -1.0)),
        ("base east", side(bp, east, 1.0)),
        ("base west", side(bp, west, -1.0)),
        ("north", tip(north, "north", p(0.0, h), &g)),
        ("south", tip(south, "south", p(0.0, -h), &g)),
        ("east", tip(east, "east", p(w, 0.0), &g)),
        ("west", tip(west, "west", p(-w, 0.0), &g)),
        ("north east", p(w, h)),
        ("north west", p(-w, h)),
        ("south west", p(-w, -h)),
        ("south east", p(w, -h)),
    ];
    for (name, q) in anchors {
        g.anchor_at(name, q);
    }
    // the anchors of a missing arrow fall back to its side
    for side in ["east", "west", "north", "south"] {
        let fallback = g.anchor(side).unwrap_or(c);
        for k in ["before {} arrow", "before {} arrow head", "before {} arrow tip", "{} arrow tip", "after {} arrow tip", "after {} arrow head", "after {} arrow"] {
            let name = k.replace("{}", side);
            if !g.has_anchor(&name) {
                g.anchor_at(&name, fallback);
            }
        }
    }
    g.background(polygon_path(&shape));
    g.roundable = true;
    g
}
