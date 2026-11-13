//! The shapes of electrical circuits: `pgflibraryshapes.gates.ee.code.tex` (`rectangle ee` and
//! `circle ee`, the rectangle and circle with an `input` and an `output` anchor, and `direction ee`,
//! an arrow tip on a line) and `pgflibraryshapes.gates.ee.IEC.code.tex` (resistors, inductors,
//! capacitors, ground, batteries, diodes, contacts).
//!
//! Most of them are a rectangle of the node's minimum size, whose `input` and `output` are its west
//! and east ends; what they draw in it is in pgf's code, by the corners without outer sep. The
//! inductor and the contacts stand on their base line instead (their center is the origin), and the
//! two generic shapes draw a mark of lines (`/pgf/generic circle IEC/tikra mark`) and run the
//! basic-layer code of a key (`/pgf/generic circle IEC/before background`) in a frame where 1pt is
//! their size.

use crate::scene::{Point, Segment, Transform};
use crate::PT;

use super::basic::{box_anchors, circle, ellipse_path, rectangle, rectangle_corners};
use super::{arc_to, Border, Geometry, Input, Layer, Usage};

/// `input` and `output`: the middle of the left and right sides of the box from `sw` to `ne`.
fn ends(g: &mut Geometry, sw: Point, ne: Point) {
    let y = 0.5 * (sw.y + ne.y);
    g.anchor_at("input", Point::new(sw.x, y));
    g.anchor_at("output", Point::new(ne.x, y));
}

pub fn rectangle_ee(input: &mut Input) -> Geometry {
    let mut g = rectangle(input);
    let (sw, ne) = rectangle_corners(input);
    ends(&mut g, sw, ne);
    g
}

pub fn circle_ee(input: &mut Input) -> Geometry {
    let mut g = circle(input);
    let (w, e) = (g.anchor("west").expect("circle"), g.anchor("east").expect("circle"));
    g.anchor_at("input", w);
    g.anchor_at("output", e);
    g
}

/// The anchors and border that the shapes inheriting from `rectangle ee` take from its saved
/// corners; nothing drawn yet.
fn ee_box(input: &Input) -> (Geometry, Point, Point) {
    let (sw, ne) = rectangle_corners(input);
    let mut g = Geometry::new(sw.lerp(ne, 0.5), Border::Rectangle(ne.minus(sw).scale(0.5)));
    box_anchors(&mut g, sw, ne, input.ex);
    ends(&mut g, sw, ne);
    (g, sw, ne)
}

/// The corners without outer sep (`\pgf@xa`, `\pgf@ya`, `\pgf@xb`, `\pgf@yb` of the shapes' code).
fn inner(input: &Input, sw: Point, ne: Point) -> (f64, f64, f64, f64) {
    (sw.x + input.outer_xsep, sw.y + input.outer_ysep, ne.x - input.outer_xsep, ne.y - input.outer_ysep)
}

fn line(a: (f64, f64), b: (f64, f64)) -> [Segment; 2] {
    [Segment::MoveTo(Point::new(a.0, a.1)), Segment::LineTo(Point::new(b.0, b.1))]
}

/// `direction ee`: the arrow tip `/pgf/direction ee arrow` at the right end of a line, in a box as
/// long as the tip and as high as the minimum height.
pub fn direction_ee(input: &mut Input) -> Geometry {
    // another tip (`direction ee arrow=angle 45`): as long as it is, and drawn as tips are
    let spec = input.keys.text("/pgf/direction ee arrow");
    let other = Some(spec.trim()).filter(|s| !s.is_empty() && *s != "direction ee").and_then(|s| input.keys.tip_shapes(s, input.line_width));
    if let Some((d, paths)) = other {
        let ne = Point::new(0.0, 0.5 * input.minimum_height);
        let sw = Point::new(-d, -0.5 * input.minimum_height);
        let mut g = Geometry::new(sw.lerp(ne, 0.5), Border::Rectangle(ne.minus(sw).scale(0.5)));
        box_anchors(&mut g, sw, ne, input.ex);
        ends(&mut g, sw, ne);
        g.parts.clear();
        g.path(Layer::BehindBackground, line((sw.x, 0.0), (ne.x, 0.0)).to_vec(), Usage::Stroke);
        for (segs, fill, stroke) in paths {
            if fill {
                g.path(Layer::BeforeBackground, segs.clone(), Usage::LineFill);
            }
            if let Some((cap, join)) = stroke {
                g.path(Layer::BeforeBackground, segs, Usage::TipStroke(cap, join));
            }
        }
        return g;
    }
    let d = input.length("/pgf/arrow options/direction ee", 4.0 * PT);
    let ne = Point::new(0.0, 0.5 * input.minimum_height);
    let sw = Point::new(-d, -0.5 * input.minimum_height);
    let mut g = Geometry::new(sw.lerp(ne, 0.5), Border::Rectangle(ne.minus(sw).scale(0.5)));
    box_anchors(&mut g, sw, ne, input.ex);
    ends(&mut g, sw, ne);
    g.parts.clear();
    g.path(Layer::BehindBackground, line((sw.x, 0.0), (ne.x, 0.0)).to_vec(), Usage::Stroke);
    // the tip of `direction ee`, its point at the origin
    let k = 0.38268343237 * d;
    let tip = vec![Segment::MoveTo(Point::new(-d, k)), Segment::LineTo(Point::new(0.0, 0.0)), Segment::LineTo(Point::new(-d, -k)), Segment::Close];
    g.path(Layer::BeforeBackground, tip, Usage::Node);
    g
}

/// `var resistor IEC`: a zigzag line through the middle.
pub fn var_resistor(input: &mut Input) -> Geometry {
    let (mut g, sw, ne) = ee_box(input);
    let (mut xa, ya, xb, yb) = inner(input, sw, ne);
    let yc = 0.5 * (ya + yb);
    let xc = 0.5 * (yb - ya);
    let mut v = vec![Segment::MoveTo(Point::new(xa, yc))];
    let end = xb + 0.1 * PT;
    // one zig and one zag per four heights, while they fit
    loop {
        xa += 4.0 * xc;
        if xa >= end || xc <= 0.0 {
            break;
        }
        xa -= 3.0 * xc;
        v.push(Segment::LineTo(Point::new(xa, yb)));
        xa += 2.0 * xc;
        v.push(Segment::LineTo(Point::new(xa, ya)));
        xa += xc;
        v.push(Segment::LineTo(Point::new(xa, yc)));
    }
    v.push(Segment::LineTo(Point::new(xb, yc)));
    g.background(v);
    g
}

/// The saved corners of the shapes that stand on their base line (`inductor IEC`, the contacts):
/// as wide as the minimum width, as high as the minimum height above the origin. `half_left`: the
/// left side has only half the outer x sep.
fn base_box(input: &Input, half_left: bool) -> (Point, Point) {
    let (ox, oy) = (input.outer_xsep, input.outer_ysep);
    let ne = Point::new(ox + 0.5 * input.minimum_width, oy + input.minimum_height);
    let sw = Point::new(-(if half_left { 0.5 } else { 1.0 }) * ox - 0.5 * input.minimum_width, -oy);
    (sw, ne)
}

/// The anchors of a shape on its base line, with its border: above the base line the rectangle of
/// `ne`, below it the one of `sw` (lowered by `drop`), both around the origin.
fn base_geometry(input: &Input, sw: Point, ne: Point, drop: f64) -> Geometry {
    let (lx, ly) = (-sw.x, -sw.y + drop);
    let outline = vec![
        Point::new(ne.x, 0.0),
        Point::new(ne.x, ne.y),
        Point::new(-ne.x, ne.y),
        Point::new(-ne.x, 0.0),
        Point::new(-lx, 0.0),
        Point::new(-lx, -ly),
        Point::new(lx, -ly),
        Point::new(lx, 0.0),
    ];
    let mut g = Geometry::new(Point::new(0.0, 0.0), Border::Outline(outline));
    g.anchor_at("center", Point::new(0.0, 0.0));
    let mut b = Geometry::new(Point::new(0.0, 0.0), Border::Point);
    box_anchors(&mut b, sw, ne, input.ex);
    for (name, p) in b.anchors {
        if ["north", "south", "east", "west", "north east", "north west", "south east", "south west"].contains(&name.as_str()) {
            g.anchor_at(&name, p);
        }
    }
    ends(&mut g, sw, ne);
    g
}

/// `inductor IEC`: half circles along the base line.
pub fn inductor(input: &mut Input) -> Geometry {
    let (sw, ne) = base_box(input, false);
    let mut g = base_geometry(input, sw, ne, 0.5 * PT);
    let xb = ne.x - input.outer_xsep;
    let yb = ne.y - input.outer_ysep;
    let mut xa = -xb;
    let mut v = vec![Segment::MoveTo(Point::new(xa, 0.0))];
    let mut at = Point::new(xa, 0.0);
    loop {
        xa += 2.0 * yb;
        if xa >= xb + 0.1 * PT || yb <= 0.0 {
            break;
        }
        at = arc_to(&mut v, at, 180.0, 0.0, yb, yb);
    }
    v.push(Segment::LineTo(Point::new(xb, 0.0)));
    g.background(v);
    g
}

/// `capacitor IEC`: the left and right sides.
pub fn capacitor(input: &mut Input) -> Geometry {
    let (mut g, sw, ne) = ee_box(input);
    let (xa, ya, xb, yb) = inner(input, sw, ne);
    g.background([line((xa, ya), (xa, yb)), line((xb, ya), (xb, yb))].concat());
    g
}

/// `ground IEC`: three lines, each a third shorter.
pub fn ground(input: &mut Input) -> Geometry {
    let (mut g, sw, ne) = ee_box(input);
    let (xa, mut ya, xb, mut yb) = inner(input, sw, ne);
    let mut v = line((xa, ya), (xa, yb)).to_vec();
    let xm = 0.5 * xa + 0.5 * xb;
    let yc = 0.16666 * yb - 0.16666 * ya;
    ya += yc;
    yb -= yc;
    v.extend(line((xm, ya), (xm, yb)));
    ya += yc;
    yb -= yc;
    v.extend(line((xb, ya), (xb, yb)));
    g.background(v);
    g
}

/// `battery IEC`: a long and a short line.
pub fn battery(input: &mut Input) -> Geometry {
    let (mut g, sw, ne) = ee_box(input);
    let (xa, mut ya, xb, mut yb) = inner(input, sw, ne);
    let mut v = line((xa, ya), (xa, yb)).to_vec();
    let yc = 0.25 * yb - 0.25 * ya;
    ya += yc;
    yb -= yc;
    v.extend(line((xb, ya), (xb, yb)));
    g.background(v);
    g
}

/// `breakdown diode IEC`: two triangles tip to tip, with a line through and a bar between them.
pub fn breakdown_diode(input: &mut Input) -> Geometry {
    let (mut g, sw, ne) = ee_box(input);
    let (xa, ya, xb, yb) = inner(input, sw, ne);
    let (xc, yc) = (0.5 * (xa + xb), 0.5 * (ya + yb));
    let p = Point::new;
    g.background(vec![
        Segment::MoveTo(p(xa, ya)),
        Segment::LineTo(p(xc, yc)),
        Segment::LineTo(p(xa, yb)),
        Segment::Close,
        Segment::MoveTo(p(xb, ya)),
        Segment::LineTo(p(xc, yc)),
        Segment::LineTo(p(xb, yb)),
        Segment::Close,
    ]);
    g.path(Layer::BeforeBackground, line((xa, yc), (xb, yc)).to_vec(), Usage::Stroke);
    let s = 0.5 * (yb - ya);
    let q = |x: f64, y: f64| p(xc + x * s, yc + y * s);
    g.path(Layer::BeforeBackground, vec![Segment::MoveTo(q(-0.5, -1.0)), Segment::LineTo(q(0.0, -1.0)), Segment::LineTo(q(0.0, 1.0)), Segment::LineTo(q(0.5, 1.0))], Usage::Stroke);
    g
}

/// What a generic shape draws in its frame at `origin`, where 1 (1pt for code) is `size`: the lines
/// of its `tikra mark` (`x,y x,y; …`, or `wave`, the sign of alternating current), then the paths
/// that the basic-layer code of its `before background` key strokes.
fn key_code(input: &mut Input, g: &mut Geometry, shape: &str, origin: Point, size: f64) {
    let p = |x: f64, y: f64| origin.plus(Point::new(x * size, y * size));
    let mark = input.keys.text(&format!("/pgf/{shape}/tikra mark"));
    let mut v = Vec::new();
    if mark.trim() == "wave" {
        // two quadratic arcs through the center, as cubic curves
        let q = |a: (f64, f64), c: (f64, f64), b: (f64, f64)| {
            Segment::CurveTo(p(a.0 + 2.0 / 3.0 * (c.0 - a.0), a.1 + 2.0 / 3.0 * (c.1 - a.1)), p(b.0 + 2.0 / 3.0 * (c.0 - b.0), b.1 + 2.0 / 3.0 * (c.1 - b.1)), p(b.0, b.1))
        };
        v.extend([Segment::MoveTo(p(0.7, 0.0)), q((0.7, 0.0), (0.35, -0.5), (0.0, 0.0)), q((0.0, 0.0), (-0.35, 0.5), (-0.7, 0.0))]);
    } else {
        for line in mark.split(';') {
            let pts: Vec<Point> =
                line.split_whitespace().filter_map(|xy| xy.split_once(',')).filter_map(|(x, y)| Some(p(x.trim().parse().ok()?, y.trim().parse().ok()?))).collect();
            for (k, q) in pts.iter().enumerate() {
                v.push(if k == 0 { Segment::MoveTo(*q) } else { Segment::LineTo(*q) });
            }
        }
    }
    if !v.is_empty() {
        g.path(Layer::BeforeBackground, v, Usage::Stroke);
    }
    let t = Transform::translation(origin.x, origin.y).compose(&Transform::scaling(size / PT, size / PT));
    for segs in input.keys.code_paths(&format!("/pgf/{shape}/before background"), t) {
        g.path(Layer::BeforeBackground, segs, Usage::Stroke);
    }
}

/// `generic circle IEC`: a circle; its key's code is run around the center, 1pt being the radius.
pub fn generic_circle(input: &mut Input) -> Geometry {
    let mut g = circle_ee(input);
    let c = g.center;
    let r = g.anchor("east").expect("circle").x - c.x - input.outer_sep();
    key_code(input, &mut g, "generic circle IEC", c, r);
    g
}

/// `generic diode IEC`: a triangle pointing right, a line through it, and its key's code at its tip,
/// 1pt being half its height.
pub fn generic_diode(input: &mut Input) -> Geometry {
    let (mut g, sw, ne) = ee_box(input);
    let (xa, ya, xb, yb) = inner(input, sw, ne);
    let yc = 0.5 * (ya + yb);
    let p = Point::new;
    g.background(vec![Segment::MoveTo(p(xa, ya)), Segment::LineTo(p(xa, yb)), Segment::LineTo(p(xb - 0.5 * input.line_width, yc)), Segment::Close]);
    g.path(Layer::BeforeBackground, line((xa, yc), (xb, yc)).to_vec(), Usage::Stroke);
    key_code(input, &mut g, "generic diode IEC", p(xb, yc), 0.5 * (yb - ya));
    g
}

/// `make contact IEC`: a line rising from the left end of the base line.
pub fn make_contact(input: &mut Input) -> Geometry {
    let (sw, ne) = base_box(input, true);
    let mut g = base_geometry(input, sw, ne, 0.5 * PT);
    let (xb, yb) = (ne.x - input.outer_xsep, ne.y - input.outer_ysep);
    g.background(line((-xb, 0.0), (xb, yb)).to_vec());
    g
}

/// `var make contact IEC`: the same line, from a small circle.
pub fn var_make_contact(input: &mut Input) -> Geometry {
    let (mut sw, ne) = base_box(input, true);
    sw.y += 0.083333 * sw.x;
    let mut g = base_geometry(input, sw, ne, 0.0);
    let (xb, yb) = (ne.x - input.outer_xsep, ne.y - input.outer_ysep);
    let r = 0.083333 * 2.0 * xb;
    let xa = -xb + r;
    let mut v = ellipse_path(Point::new(xa, 0.0), r, r);
    let d = Point::new(xb - xa, yb);
    let u = d.scale(1.0 / d.norm().max(1e-12));
    v.extend([Segment::MoveTo(Point::new(xa, 0.0).plus(u.scale(r))), Segment::LineTo(Point::new(xb, yb))]);
    g.background(v);
    g
}

/// `break contact IEC`: the make contact's line resting on a hook at the right.
pub fn break_contact(input: &mut Input) -> Geometry {
    let (sw, ne) = base_box(input, true);
    let mut g = base_geometry(input, sw, ne, 0.5 * PT);
    let (xb, yb) = (ne.x - input.outer_xsep, ne.y - input.outer_ysep);
    let xa = -xb;
    let xc = 0.2 * xb - 0.2 * xa;
    let mut v = line((xa, 0.0), (xb, yb)).to_vec();
    v.extend([Segment::MoveTo(Point::new(xb, 0.0)), Segment::LineTo(Point::new(xb - xc, 0.0)), Segment::LineTo(Point::new(xb - xc, yb))]);
    g.background(v);
    g
}
