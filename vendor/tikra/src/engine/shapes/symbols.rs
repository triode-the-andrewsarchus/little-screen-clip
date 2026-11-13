//! The shapes of `pgflibraryshapes.symbols.code.tex`: forbidden signs, magnifying glass, starburst,
//! cloud, signal, tape and magnetic tape.

use std::f64::consts::{FRAC_1_SQRT_2, SQRT_2};

use crate::scene::{Point, Segment};

use super::{angle_between, arc_points, arc_to, basic, cos, flatten, miter_offset, polar, polygon_path, ray, rotate_around, sin, tan, Border, Geometry, Input, Layer, Usage};

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

fn text_anchors(g: &mut Geometry, input: &Input) {
    g.anchor_at("center", input.center_point());
    g.anchor_at("mid", input.mid_point());
    g.anchor_at("base", input.base_point());
}

/// The circle's radius (without outer sep) and center, for the shapes that are nearly circles.
fn circle_line(input: &mut Input, from: f64, to: f64, angle: f64) -> Geometry {
    let mut g = basic::circle(input);
    let (xa, ya) = input.half_with_inner();
    let r = xa.hypot(ya).max(input.minimum_width / 2.0).max(input.minimum_height / 2.0);
    let c = g.center;
    let v = vec![Segment::MoveTo(c.plus(polar(angle, from * r))), Segment::LineTo(c.plus(polar(angle, to * r)))];
    g.path(Layer::Foreground, v, Usage::Node);
    g
}

/// A circle crossed from south west to north east.
pub fn forbidden_sign(input: &mut Input) -> Geometry {
    circle_line(input, -1.0, 1.0, 45.0)
}

/// A circle crossed from south east to north west.
pub fn correct_forbidden_sign(input: &mut Input) -> Geometry {
    circle_line(input, -1.0, 1.0, 135.0)
}

/// A circle with a handle.
pub fn magnifying_glass(input: &mut Input) -> Geometry {
    let angle = input.number("/pgf/magnifying glass handle angle", -45.0);
    let aspect = input.number("/pgf/magnifying glass handle aspect", 1.5);
    circle_line(input, 1.0, 1.0 + aspect, angle)
}

/// pgf's pseudo-random numbers (`\pgfmathgeneratepseudorandomnumber`, `rnd`), from a seed.
struct Random(i64);

impl Random {
    fn next(&mut self) -> f64 {
        let (a, q, r, m) = (69621i64, 30845i64, 23902i64, 2147483647i64);
        let z = self.0;
        let mut n = a * (z % q) - r * (z / q);
        if n < 0 {
            n += m;
        }
        self.0 = n;
        (n % 100001) as f64 / 100000.0
    }
}

pub fn starburst(input: &mut Input) -> Geometry {
    let points = input.number("/pgf/starburst points", 17.0).trunc().max(2.0);
    let step = 180.0 / points;
    let total = (2.0 * points) as usize;
    let height = input.length("/pgf/starburst point height", 0.5);
    let seed = input.number("/pgf/random starburst", 100.0).trunc() as i64;
    let (mut x, mut y) = input.half_with_inner();
    let (rotate, incircle) = input.border_rotation();
    if incircle {
        x = SQRT_2 * x.max(y);
        y = x;
    } else {
        if rotate == 90.0 || rotate == 270.0 {
            (x, y) = (y, x);
        }
        x *= SQRT_2;
        y *= SQRT_2;
    }
    if x + height < input.minimum_width / 2.0 {
        x = input.minimum_width / 2.0 - height;
    }
    if y + height < input.minimum_height / 2.0 {
        y = input.minimum_height / 2.0 - height;
    }
    let c = input.center_point();
    let mut random = Random(seed);
    let shape: Vec<Point> = (0..total)
        .map(|k| {
            let angle = 90.0 + k as f64 * step;
            let h = if k % 2 == 1 {
                0.0
            } else if seed == 0 {
                height
            } else {
                0.75 * height * random.next() + 0.25 * height
            };
            c.plus(Point::new((x + h) * cos(angle), (y + h) * sin(angle)))
        })
        .collect();
    // pgf rotates the border, not the drawn burst
    let border: Vec<Point> = miter_offset(&shape, input.outer_sep()).into_iter().map(|p| rotate_around(p, c, rotate)).collect();
    let mut g = Geometry::new(c, Border::Outline(border.clone()));
    text_anchors(&mut g, input);
    compass(&mut g);
    for k in 0..points as usize {
        g.anchor_at(&format!("outer point {}", k + 1), border[2 * k]);
        g.anchor_at(&format!("inner point {}", k + 1), border[2 * k + 1]);
    }
    g.background(polygon_path(&shape));
    g.roundable = true;
    g
}

/// A cloud's puffs: the background, the outline with outer sep and the puff anchors, and the
/// radii of the ellipse through the puffs' extremities.
pub struct Cloud {
    pub background: Vec<Segment>,
    pub outline: Vec<Point>,
    pub puffs: Vec<Point>,
    pub outer: Point,
    /// the radii of the ellipse through the puffs' ends
    pub inner: Point,
}

/// The puffs of a cloud around `c` whose inner ellipse has radii `(xi, yi)`.
fn puffs(c: Point, xi: f64, yi: f64, n: usize, arc: f64, o: f64) -> (Vec<Segment>, Vec<Point>, Vec<Point>) {
    let step = 360.0 / n as f64;
    let quotient = 0.5 / cos((180.0 - arc) / 2.0);
    let half_complement = (180.0 - arc) / 2.0;
    let quarter = arc / 4.0;
    let k = 0.55228475;
    let on = |angle: f64| c.plus(Point::new(xi * cos(angle), yi * sin(angle)));
    let start = 90.0 - step / 2.0;
    let first = on(start);
    let mut v = vec![Segment::MoveTo(first)];
    let (mut outline, mut anchors) = (Vec::new(), Vec::new());
    for i in 0..n {
        let a0 = start + i as f64 * step;
        let (s, e) = (on(a0), if i + 1 == n { first } else { on(a0 + step) });
        // the puff's circle, in the frame of its chord (from its end to its start)
        let slope = angle_between(e, s);
        let chord = e.minus(s).norm();
        let half = chord / 2.0;
        let r = quotient * chord;
        let seg = r - sin(half_complement) * r;
        let rot = |p: Point, a: f64| rotate_around(p, Point::new(0.0, 0.0), a);
        let center = s.plus(rot(Point::new(-half, seg - r), slope));
        let mid = s.plus(rot(Point::new(-half, seg), slope));
        let scale = r * tan(quarter);
        let control = |base: Point, sign: f64, turn: f64| {
            let local = Point::new(k * sin(quarter) * scale, sign * k * cos(quarter) * scale);
            base.plus(rot(local, turn))
        };
        let turn1 = 90.0 - quarter + slope;
        let turn2 = quarter + 90.0 + slope;
        v.push(Segment::CurveTo(control(s, 1.0, turn1), control(mid, -1.0, turn1), mid));
        v.push(Segment::CurveTo(control(mid, 1.0, turn2), control(e, -1.0, turn2), e));
        let ro = r + o;
        outline.extend(arc_points(center, ro, ro, slope + half_complement, slope + half_complement + arc));
        anchors.push(center.plus(polar(slope + 90.0, ro)));
    }
    v.push(Segment::Close);
    (v, outline, anchors)
}

/// The cloud of the symbols library (also the body of `cloud callout`).
pub fn cloud_parts(input: &mut Input) -> Cloud {
    let n = input.number("/pgf/cloud puffs", 10.0).trunc().max(2.0) as usize;
    let arc = input.number("/pgf/cloud puff arc", 150.0);
    let step = 360.0 / n as f64;
    let (x, y) = input.half_with_inner();
    let (x, y) = (SQRT_2 * x, SQRT_2 * y);
    let (xc, yc) = if input.keys.flag("tikra@sh@cloudnoaspect") {
        (x, y)
    } else {
        let aspect = input.aspect;
        let xc = (aspect * y).max(x);
        // (the height the aspect gives may round a hair below the text's)
        let low = xc / aspect < y;
        (if low { aspect * y } else { xc }, if low { y } else { xc / aspect })
    };
    let half_arc = arc / 2.0;
    let k = sin(step / 2.0) * (1.0 - cos(half_arc)) / sin(half_arc);
    let ch = cos(step / 2.0);
    // the ellipse through the puffs' extremities, at least the minimum size
    let big_x = (ch * xc + k * yc).max(input.minimum_width / 2.0);
    let big_y = (ch * yc + k * xc).max(input.minimum_height / 2.0);
    let d = ch * ch - k * k;
    let xi = (ch * big_x - k * big_y) / d;
    let yi = (ch * big_y - k * big_x) / d;
    let (background, outline, puff_anchors) = puffs(input.center_point(), xi, yi, n, arc, input.outer_sep());
    Cloud { background, outline, puffs: puff_anchors, outer: Point::new(big_x, big_y), inner: Point::new(xi, yi) }
}

pub fn cloud(input: &mut Input) -> Geometry {
    let cl = cloud_parts(input);
    let c = input.center_point();
    let border = if input.keys.flag("tikra@sh@cloudellipse") { Border::Ellipse(cl.outer) } else { Border::Outline(cl.outline) };
    let mut g = Geometry::new(c, border);
    text_anchors(&mut g, input);
    let (x, y) = (cl.outer.x, cl.outer.y);
    let k = FRAC_1_SQRT_2;
    for (name, d) in [
        ("north", (0.0, y)),
        ("south", (0.0, -y)),
        ("east", (x, 0.0)),
        ("west", (-x, 0.0)),
        ("north west", (-k * x, k * y)),
        ("north east", (k * x, k * y)),
        ("south west", (-k * x, -k * y)),
        ("south east", (k * x, -k * y)),
    ] {
        let p = g.border_point(Point::new(d.0, d.1));
        g.anchor_at(name, p);
    }
    for (i, p) in cl.puffs.iter().enumerate() {
        g.anchor_at(&format!("puff {}", i + 1), *p);
    }
    g.background(cl.background);
    g
}

#[derive(Clone, Copy, PartialEq)]
enum Pointer {
    Nowhere,
    From,
    To,
}

/// `signal from` and `signal to`: which sides point in and out.
fn signal_sides(from: &str, to: &str) -> [Pointer; 4] {
    // north, south, east, west
    let mut sides = [Pointer::Nowhere; 4];
    for (text, kind) in [(from, Pointer::From), (to, Pointer::To)] {
        let (a, b) = match text.split_once(" and ") {
            Some((a, b)) => (a.trim().to_string(), b.trim().to_string()),
            None => (text.trim().to_string(), text.trim().to_string()),
        };
        if a.contains("nowhere") {
            continue;
        }
        for w in [a, b] {
            match w.as_str() {
                "east" | "right" => {
                    sides[2] = kind;
                    sides[0] = Pointer::Nowhere;
                    sides[1] = Pointer::Nowhere;
                }
                "west" | "left" => {
                    sides[3] = kind;
                    sides[0] = Pointer::Nowhere;
                    sides[1] = Pointer::Nowhere;
                }
                "north" | "above" | "up" => {
                    sides[0] = kind;
                    sides[2] = Pointer::Nowhere;
                    sides[3] = Pointer::Nowhere;
                }
                "south" | "below" | "down" => {
                    sides[1] = kind;
                    sides[2] = Pointer::Nowhere;
                    sides[3] = Pointer::Nowhere;
                }
                _ => {}
            }
        }
    }
    sides
}

pub fn signal(input: &mut Input) -> Geometry {
    let from = input.keys.text("/pgf/signal from");
    let to = input.keys.text("/pgf/signal to");
    let [n, s, e, w] = signal_sides(&from, &to);
    let half = input.number("/pgf/signal pointer angle", 90.0) / 2.0;
    let (mut xa, mut ya) = input.half_with_inner();
    let t = tan(90.0 - half);
    let (mut xb, mut yb) = (t * ya, t * xa);
    let pointed = |p: Pointer| if p == Pointer::Nowhere { 0.0 } else { 1.0 };
    let tall = 2.0 * ya + (pointed(n) + pointed(s)) * yb;
    if tall < input.minimum_height {
        ya = 0.5 * (input.minimum_height - (pointed(n) + pointed(s)) * yb);
        xb = t * ya;
    }
    let wide = 2.0 * xa + (pointed(e) + pointed(w)) * xb;
    if wide < input.minimum_width {
        xa = 0.5 * (input.minimum_width - (pointed(e) + pointed(w)) * xb);
        yb = t * xa;
    }
    let c = input.center_point();
    let p = |dx: f64, dy: f64| c.plus(Point::new(dx, dy));
    let out = |side: Pointer, kind: Pointer, d: f64| if side == kind { d } else { 0.0 };
    let shape = [
        p(0.0, ya + out(n, Pointer::To, yb)),
        p(xa + out(e, Pointer::From, xb), ya + out(n, Pointer::From, yb)),
        p(xa + out(e, Pointer::To, xb), 0.0),
        p(xa + out(e, Pointer::From, xb), -ya - out(s, Pointer::From, yb)),
        p(0.0, -ya - out(s, Pointer::To, yb)),
        p(-xa - out(w, Pointer::From, xb), -ya - out(s, Pointer::From, yb)),
        p(-xa - out(w, Pointer::To, xb), 0.0),
        p(-xa - out(w, Pointer::From, xb), ya + out(n, Pointer::From, yb)),
    ];
    let b = miter_offset(&shape, input.outer_sep());
    let mut g = Geometry::new(c, Border::Outline(b.clone()));
    text_anchors(&mut g, input);
    for (name, q) in [("north", b[0]), ("north east", b[1]), ("east", b[2]), ("south east", b[3]), ("south", b[4]), ("south west", b[5]), ("west", b[6]), ("north west", b[7])] {
        g.anchor_at(name, q);
    }
    for (name, o, d) in
        [("mid west", input.mid_point(), -1.0), ("mid east", input.mid_point(), 1.0), ("base west", input.base_point(), -1.0), ("base east", input.base_point(), 1.0)]
    {
        g.anchor_at(name, ray(o, Point::new(d, 0.0), &b));
    }
    g.background(polygon_path(&shape));
    g.roundable = true;
    g
}

#[derive(Clone, Copy, PartialEq)]
enum Bend {
    InAndOut,
    OutAndIn,
    None,
}

fn bend(s: &str) -> Bend {
    match s.split_whitespace().collect::<Vec<_>>().join(" ").as_str() {
        "in and out" => Bend::InAndOut,
        "out and in" => Bend::OutAndIn,
        _ => Bend::None,
    }
}

pub fn tape(input: &mut Input) -> Geometry {
    let top = bend(&input.keys.text("/pgf/tape bend top"));
    let bottom = bend(&input.keys.text("/pgf/tape bend bottom"));
    let hb = input.length("/pgf/tape bend height", 5.0 * crate::PT) / 2.0;
    let (ox, oy) = (input.outer_xsep, input.outer_ysep);
    let (mut x, mut y) = input.half_with_inner();
    // pgf always counts the bottom bend here (it compares with an undefined macro)
    if top != Bend::None {
        y += hb;
    }
    y += hb;
    x = x.max(input.minimum_width / 2.0);
    y = y.max(input.minimum_height / 2.0);
    if top != Bend::None {
        y -= hb;
    }
    if bottom != Bend::None {
        y -= hb;
    }
    let (hw, hh) = (x, y);
    let ry = 3.414213 * hb;
    let rx = FRAC_1_SQRT_2 * x;
    let half_angle = (ry / rx).atan().to_degrees() / 2.0;
    let (cot_in, cot_out) = (1.0 / tan(45.0 - half_angle), 1.0 / tan(90.0 - half_angle));
    let c = input.center_point();
    let p = |dx: f64, dy: f64| c.plus(Point::new(dx, dy));
    let mut v = vec![Segment::MoveTo(p(-hw, 0.0)), Segment::LineTo(p(-hw, hh))];
    match top {
        Bend::None => v.push(Segment::LineTo(p(hw, hh))),
        kind => {
            let q = p(-hw, hh + hb);
            v.push(Segment::LineTo(q));
            let (first, second) = if kind == Bend::InAndOut { ((225.0, 315.0), (135.0, 45.0)) } else { ((135.0, 45.0), (225.0, 315.0)) };
            let q = arc_to(&mut v, q, first.0, first.1, rx, ry);
            arc_to(&mut v, q, second.0, second.1, rx, ry);
        }
    }
    v.push(Segment::LineTo(p(hw, -hh)));
    match bottom {
        Bend::None => v.push(Segment::LineTo(p(-hw, -hh))),
        kind => {
            let q = p(hw, -hh - hb);
            v.push(Segment::LineTo(q));
            let (first, second) = if kind == Bend::InAndOut { ((45.0, 135.0), (315.0, 225.0)) } else { ((315.0, 225.0), (45.0, 135.0)) };
            let q = arc_to(&mut v, q, first.0, first.1, rx, ry);
            arc_to(&mut v, q, second.0, second.1, rx, ry);
        }
    }
    v.push(Segment::Close);
    let outline = miter_offset(&dedup(flatten(&v)), input.outer_sep());
    let mut g = Geometry::new(c, Border::Outline(outline));
    let ow = hw + ox;
    let rise = |kind: Bend, near: f64, far: f64| match kind {
        Bend::InAndOut => hb + near * oy,
        Bend::OutAndIn => hb + far * oy,
        Bend::None => oy,
    };
    let ne = p(ow, hh + rise(top, cot_out, cot_in));
    let nw = p(-ow, hh + rise(top, cot_in, cot_out));
    // pgf takes the south corners from the top bend too
    let se = p(ow, -hh - rise(top, cot_in, cot_out));
    let sw = p(-ow, -hh - rise(top, cot_out, cot_in));
    let (m, b) = (input.mid_point(), input.base_point());
    for (name, q) in [
        ("center", c),
        ("mid", m),
        ("base", b),
        ("mid east", m.plus(Point::new(ow, 0.0))),
        ("mid west", m.plus(Point::new(-ow, 0.0))),
        ("base east", b.plus(Point::new(ow, 0.0))),
        ("base west", b.plus(Point::new(-ow, 0.0))),
        ("north", Point::new(c.x, (ne.y + nw.y) / 2.0)),
        ("south", Point::new(c.x, (se.y + sw.y) / 2.0)),
        ("east", p(ow, 0.0)),
        ("west", p(-ow, 0.0)),
        ("north east", ne),
        ("north west", nw),
        ("south east", se),
        ("south west", sw),
    ] {
        g.anchor_at(name, q);
    }
    g.background(v);
    g
}

/// The points without repeats (a flattened path's joints), for offsetting.
fn dedup(points: Vec<Point>) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    for p in points {
        if out.last().is_none_or(|q| q.minus(p).norm() > 1e-9) {
            out.push(p);
        }
    }
    if out.len() > 1 && out[0].minus(*out.last().unwrap()).norm() <= 1e-9 {
        out.pop();
    }
    out
}

pub fn magnetic_tape(input: &mut Input) -> Geometry {
    let t = input.text();
    let size = (t.wd + 2.0 * input.inner_xsep).max(t.ht + t.dp + 2.0 * input.inner_ysep) * SQRT_2;
    let r = size.max(input.minimum_width).max(input.minimum_height) / 2.0;
    let extend = input.length("/pgf/magnetic tape tail extend", 0.0).max(0.0);
    let tail = input.number("/pgf/magnetic tape tail", 0.15).clamp(0.0, 1.0) * r;
    let o = input.outer_sep();
    let big = r + o;
    let tail_angle = 360.0 - ((r - tail) / r).clamp(-1.0, 1.0).asin().to_degrees();
    let c = input.center_point();
    let p = |dx: f64, dy: f64| c.plus(Point::new(dx, dy));
    let start = c.plus(polar(tail_angle, r));
    let mut v = vec![Segment::MoveTo(start)];
    let q = arc_to(&mut v, start, tail_angle, 360.0, r, r);
    arc_to(&mut v, q, 0.0, 270.0, r, r);
    v.push(Segment::LineTo(p(r + extend, -r)));
    v.push(Segment::LineTo(p(r + extend, -r + tail)));
    v.push(Segment::Close);
    // the border: the circle, and the tail's bottom and end
    let top = 360.0 - ((big - 2.0 * o - tail) / (big + extend)).atan().to_degrees();
    let mut outline = arc_points(c, big, big, top - 360.0, 270.0);
    outline.push(p(big + extend, -big));
    outline.push(p(big + extend, -big + 2.0 * o + tail));
    let mut g = Geometry::new(c, Border::Outline(outline));
    let (b, m) = (input.base_point(), input.mid_point());
    let tx = r + extend + o;
    for (name, q) in [
        ("center", c),
        ("base", b),
        ("mid", m),
        ("base west", b.plus(Point::new(-big, 0.0))),
        ("base east", b.plus(Point::new(big, 0.0))),
        ("mid west", m.plus(Point::new(-big, 0.0))),
        ("mid east", m.plus(Point::new(big, 0.0))),
        ("north", p(0.0, big)),
        ("north east", c.plus(polar(45.0, big))),
        ("south", p(0.0, -big)),
        ("south east", p(big, -big)),
        ("east", p(big, 0.0)),
        ("south west", c.plus(polar(225.0, big))),
        ("west", p(-big, 0.0)),
        ("north west", c.plus(polar(135.0, big))),
        ("tail east", p(tx, -big + o + tail / 2.0)),
        ("tail south east", p(tx, -big)),
        ("tail north east", p(tx, -big + 2.0 * o + tail)),
    ] {
        g.anchor_at(name, q);
    }
    g.background(v);
    g
}
