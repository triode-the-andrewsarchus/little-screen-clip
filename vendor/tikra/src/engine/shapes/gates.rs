//! The logic gates of `pgflibraryshapes.gates.logic.US.code.tex` (and, nand, or, nor, xor, xnor, not
//! and buffer, drawn the American way, and the and and nand gates of the CDH style) and of
//! `pgflibraryshapes.gates.logic.IEC.code.tex` (the same gates as rectangles with a symbol), with the
//! keys they share from `pgflibraryshapes.gates.logic.code.tex`.
//!
//! A gate's size grows with its text and with its number of inputs (`/pgf/logic gate inputs`, `nin`
//! or `normal,inverted,normal`): the inputs are spread evenly along its back, `input 1` at the top,
//! and an inverted one gets a small circle of `/pgf/logic gate inverted radius`. The US gates are
//! square around their text, by their `\halfside`; their arcs and anchors are pgf's constants times
//! it. pgf computes their border piece by piece (lines and arcs, depending on the direction); here
//! the same pieces are sampled into an outline around the center.

use crate::scene::{Point, Segment};
use crate::PT;

use super::basic::ellipse_path;
use super::{arc_to, intersect_lines, polar, Border, Geometry, Input, ShapeText};

/// `\pgf@lib@sh@logicgate@parseinputs`: whether each of the first `max` inputs is inverted. Without a
/// comma every letter is an input (`i` inverted); with commas every item is (`inverted`).
pub fn parse_inputs(spec: &str, max: usize) -> Vec<bool> {
    let v: Vec<bool> = if spec.contains(',') {
        spec.split(',').map(|s| s.trim_start() == "inverted").collect()
    } else {
        spec.chars().filter(|c| !c.is_whitespace() && *c != '{' && *c != '}').map(|c| c == 'i').collect()
    };
    v.into_iter().take(max).collect()
}

/// What every gate saves: its inputs, the inverted radius, `\outerinvertedradius` (with half the
/// line width) and the center of its text.
struct Common {
    inputs: Vec<bool>,
    r: f64,
    outer_r: f64,
    sep: f64,
    lw: f64,
    c: Point,
}

fn common(input: &mut Input, max: usize) -> Common {
    let spec = input.keys.text("/pgf/logic gate inputs");
    let inputs = parse_inputs(&spec, max);
    let r = input.length("/pgf/logic gate inverted radius", 2.0 * PT);
    let sep = input.length("/pgf/logic gate input sep", 0.125);
    Common { inputs, r, outer_r: r + 0.5 * input.line_width, sep, lw: input.line_width, c: input.center_point() }
}

/// The anchors every gate has from its text: `center`, `mid`, `base`, `text`.
fn text_anchors(g: &mut Geometry, input: &Input) {
    g.anchor_at("center", input.center_point());
    g.anchor_at("mid", input.mid_point());
    g.anchor_at("base", input.base_point());
}

/// `mid east`, `base east`… : the x of an anchor at the height of `mid` and `base`.
fn side_anchors(g: &mut Geometry, input: &Input, side: &str, x: f64) {
    g.anchor_at(&format!("mid {side}"), Point::new(x, input.mid_point().y));
    g.anchor_at(&format!("base {side}"), Point::new(x, 0.0));
}

/// Where the ray from `o` along `d` meets the arc of radii `rx`, `ry` around `m` between `a0` and
/// `a1` degrees (`\pgfmathpointintersectionoflineandarc`, for a line through the center).
fn ray_arc(o: Point, d: Point, m: Point, a0: f64, a1: f64, rx: f64, ry: f64) -> Point {
    // o + t d on the ellipse: a t² + b t + c = 0
    let w = o.minus(m);
    let (ax, ay) = (d.x / rx, d.y / ry);
    let (wx, wy) = (w.x / rx, w.y / ry);
    let a = ax * ax + ay * ay;
    let b = 2.0 * (ax * wx + ay * wy);
    let c = wx * wx + wy * wy - 1.0;
    // (a negative discriminant has no root: the ray misses, and the answer is `o`)
    let disc = b * b - 4.0 * a * c;
    let (lo, hi) = (a0.min(a1), a0.max(a1));
    let within = |p: Point| {
        let q = p.minus(m);
        let ang = (q.y / ry).atan2(q.x / rx).to_degrees();
        (-1..=1).map(|k| ang + 360.0 * k as f64).any(|x| x >= lo - 1e-6 && x <= hi + 1e-6)
    };
    let mut roots: Vec<f64> = [(-b - disc.sqrt()) / (2.0 * a), (-b + disc.sqrt()) / (2.0 * a)].into_iter().filter(|t| *t > 0.0).collect();
    roots.sort_by(|x, y| x.total_cmp(y));
    let pts: Vec<Point> = roots.iter().map(|t| o.plus(d.scale(*t))).collect();
    pts.iter().copied().find(|p| within(*p)).or_else(|| pts.last().copied()).unwrap_or(o)
}

/// The ray from `o` along `d` against the line through `a` and `b`.
fn ray_line(o: Point, d: Point, a: Point, b: Point) -> Point {
    intersect_lines(o, o.plus(d), a, b).unwrap_or(o)
}

/// The border as pgf's border code gives it for each direction, sampled every half degree.
fn sampled(f: impl Fn(Point) -> Point) -> Border {
    Border::Outline((0..720).map(|k| f(polar(k as f64 * 0.5, 1.0))).collect())
}

/// The angle of `q` seen from `p`, in [0, 360).
fn angle(p: Point, q: Point) -> f64 {
    super::angle_between(p, q)
}

/// Circles of the inverted inputs: at height `top − k · 2·top/(n+1)`, at the x `x(y)`.
fn input_circles(v: &mut Vec<Segment>, cm: &Common, top: f64, x: impl Fn(f64) -> f64) {
    let n = cm.inputs.len();
    let step = 2.0 * top / (n + 1) as f64;
    for (k, inv) in cm.inputs.iter().enumerate() {
        let y = top - (k + 1) as f64 * step;
        if *inv {
            v.extend(ellipse_path(cm.c.plus(Point::new(x(y), y)), cm.r, cm.r));
        }
    }
}

/// The input anchors `input 1`… (`input` alone for the gates with one): at `y_k` and `x(k, y_k)`.
fn input_anchors(g: &mut Geometry, cm: &Common, single: bool, half: f64, x: impl Fn(bool, f64) -> f64) {
    let n = cm.inputs.len();
    let step = 2.0 * half / (n + 1) as f64;
    for (k, inv) in cm.inputs.iter().enumerate() {
        let y = half - (k + 1) as f64 * step;
        let mut dx = x(*inv, y);
        if *inv {
            dx -= cm.r + cm.outer_r;
        }
        let name = if single { "input".to_string() } else { format!("input {}", k + 1) };
        g.anchor_at(&name, cm.c.plus(Point::new(dx, y)));
    }
}

// ------------------------------------------------------------------------------------------ US

/// The US gates' `\dimensions`: `\halfside`, `\halfwidth`, `\halfheight` and (but for the and gates)
/// `\tipanchor`, relative to the center.
struct Us {
    h: f64,
    w: f64,
    hh: f64,
    ox: f64,
    oy: f64,
    tip: Point,
}

fn us_dimensions(input: &Input, cm: &Common, and: bool) -> Us {
    let t = input.text();
    let mut x = input.inner_xsep + 0.5 * t.wd;
    let mut y = input.inner_ysep + 0.5 * (t.ht + t.dp);
    let n1 = (cm.inputs.len() + 1) as f64;
    let ya = (cm.r + cm.lw).max(0.5 * cm.sep) * n1;
    y = y.max(ya);
    x = x.max(y);
    y = x;
    y = y.max(0.5 * input.minimum_height);
    if and {
        if 2.5 * x < input.minimum_width {
            x = 0.4 * input.minimum_width;
        }
    } else if x + 1.732051 * y < input.minimum_width {
        x = 0.366025 * input.minimum_width;
    }
    let h = x.max(y);
    let (ox, oy) = (input.outer_xsep, input.outer_ysep);
    let tip = Point::new(-0.16666 * h + 0.866025 * 2.0 * h + 1.154701 * ox, 0.0);
    Us { h, w: h + ox, hh: h + oy, ox, oy, tip }
}

/// The or gate's front: two arcs of radius `2·halfside` from the top to the tip and back down.
fn or_front(v: &mut Vec<Segment>, c: Point, xc: f64, yc: f64) {
    let start = c.plus(Point::new(-0.16666 * xc, yc));
    v.push(Segment::MoveTo(start));
    let p = arc_to(v, start, 90.0, 30.0, 2.0 * yc, 2.0 * yc);
    arc_to(v, p, -30.0, -90.0, 2.0 * yc, 2.0 * yc);
}

/// The front of the or, xor and CDH and gates' borders: the arcs from the top and the bottom to the
/// tip, from the center `c` along `d` (absolute).
fn or_front_border(c: Point, d: Point, u: &Us) -> Point {
    let (h, w, hh) = (u.h, u.w, u.hh);
    if d.y.abs() < 1e-12 {
        // and gate US's east anchor, with these dimensions
        return Point::new(c.x + (w - u.ox) / 3.0 + w, c.y);
    }
    if d.y < 0.0 {
        ray_arc(c, d, c.plus(Point::new(-0.166666 * h, h)), 270.0, 330.0, hh + h, hh + h)
    } else {
        ray_arc(c, d, c.plus(Point::new(-0.166666 * h, -h)), 30.0, 90.0, w + h, hh + h)
    }
}

/// The or gate's `north east` and `south east` without the bounding box.
fn or_corner(c: Point, u: &Us, s: f64) -> Point {
    Point::new(c.x - 0.166666 * u.h + 0.5 * (u.w + u.h), c.y + s * (-u.h + 0.866025 * (u.hh + u.h)))
}

// pgf's constants as it writes them (`.707106`), not the exact ones
#[allow(clippy::approx_constant)]
pub fn and_gate_us(input: &mut Input, nand: bool) -> Geometry {
    let cm = common(input, 1024);
    let u = us_dimensions(input, &cm, true);
    let c = cm.c;
    let (h, w, hh, ox) = (u.h, u.w, u.hh, u.ox);
    let bb = input.keys.flag("tikra@gatebox");
    let east = Point::new(c.x + (w - ox) / 3.0 + w, c.y);
    let nw = Point::new(c.x - 1.16666 * h - ox, c.y + hh);
    let sw = Point::new(nw.x, c.y - hh);
    let arc_top = c.plus(Point::new((w - ox) / 3.0, hh));
    let arc_bottom = c.plus(Point::new((w - ox) / 3.0, -hh));
    let border = sampled(|d| {
        let q = c.plus(d);
        let a = angle(c, q);
        let m = c.plus(Point::new(h / 3.0, 0.0));
        if a < angle(c, nw) {
            if a < angle(c, arc_top) {
                ray_arc(c, d, m, 0.0, 90.0, w, hh)
            } else {
                ray_line(c, d, arc_top, nw)
            }
        } else if a > angle(c, arc_bottom) {
            ray_arc(c, d, m, 270.0, 360.0, w, hh)
        } else if a > angle(c, sw) {
            ray_line(c, d, arc_bottom, sw)
        } else {
            ray_line(c, d, nw, sw)
        }
    });
    let mut g = Geometry::new(c, border);
    text_anchors(&mut g, input);
    let corner = |s: f64| if bb { Point::new(east.x, c.y + s * hh) } else { Point::new(c.x + (w - ox) / 3.0 + 0.707106 * w, c.y + s * 0.707106 * hh) };
    for (name, p) in [
        ("east", east),
        ("north east", corner(1.0)),
        ("south east", corner(-1.0)),
        ("north", Point::new(c.x, c.y + hh)),
        ("south", Point::new(c.x, c.y - hh)),
        ("north west", nw),
        ("south west", sw),
        ("west", Point::new(nw.x, c.y)),
    ] {
        g.anchor_at(name, p);
    }
    side_anchors(&mut g, input, "east", east.x);
    side_anchors(&mut g, input, "west", nw.x);
    let output = if nand { Point::new(c.x + 1.33333 * h + cm.r + cm.outer_r + ox, c.y) } else { east };
    g.anchor_at("output", output);
    input_anchors(&mut g, &cm, false, h, |_, _| -0.166666 * h - w);
    // the background: a half circle in front of a square
    let (xc, yc) = (w - ox, hh - u.oy);
    let mut v = Vec::new();
    let start = c.plus(Point::new(xc / 3.0, yc));
    v.push(Segment::MoveTo(start));
    arc_to(&mut v, start, 90.0, -90.0, yc, yc);
    v.push(Segment::LineTo(c.plus(Point::new(-1.16666 * xc, -yc))));
    v.push(Segment::LineTo(c.plus(Point::new(-1.16666 * xc, yc))));
    v.push(Segment::Close);
    input_circles(&mut v, &cm, yc, |_| -1.16666 * h - 0.5 * cm.lw - cm.r);
    if nand {
        v.extend(ellipse_path(c.plus(Point::new(1.33333 * xc + cm.outer_r, 0.0)), cm.r, cm.r));
    }
    g.background(v);
    g
}

/// The x of an or gate's input at height `y`, on its back arc.
fn or_input_x(u: &Us, lw: f64, y: f64) -> f64 {
    let (h, yb) = (u.h, 2.0 * u.h - 0.5 * lw);
    let cosv = (y / yb).clamp(-1.0, 1.0).asin().cos();
    -1.166666 * h - 0.866025 * 2.0 * h + cosv * yb
}

/// `or gate US`, `nor gate US`, and the xor and xnor gates with their second back line.
pub fn or_gate_us(input: &mut Input, kind: &str) -> Geometry {
    let xor = kind.starts_with('x');
    let inverted = kind == "nor" || kind == "xnor";
    let cm = common(input, if xor { 2 } else { 1024 });
    let u = us_dimensions(input, &cm, false);
    let c = cm.c;
    let (h, w, hh, ox, lw) = (u.h, u.w, u.hh, u.ox, cm.lw);
    let bb = input.keys.flag("tikra@gatebox");
    let east = c.plus(u.tip);
    // the corners at the back: the or gate's are on its arc, the xor gate's on its second line
    let back = |bb: bool, s: f64| {
        if xor {
            let xb = 2.0 * h - 0.5 * lw;
            Point::new(c.x - 3.232051 * h + 0.866025 * xb, c.y + s * if bb { hh } else { 0.5 * xb })
        } else {
            Point::new(c.x - 1.16666 * h - 1.732050 * ox, c.y + s * hh)
        }
    };
    let border = {
        let nw_bb = back(true, 1.0);
        let sw_bb = back(true, -1.0);
        let (north, south) = (Point::new(c.x, c.y + hh), Point::new(c.x, c.y - hh));
        let shift = if xor { 0.166666 * 2.0 * h } else { 0.0 };
        sampled(|d| {
            if d.x < 0.0 {
                let a = angle(c, c.plus(d));
                let top = angle(c, if xor { nw_bb } else { back(false, 1.0) });
                if a < top {
                    ray_line(c, d, north, nw_bb)
                } else if a > 360.0 - top {
                    ray_line(c, d, south, sw_bb)
                } else {
                    let m = Point::new(c.x - 1.166666 * h - 0.866025 * 2.0 * h - ox - shift, c.y);
                    let r = hh + h - ox;
                    if d.y > 0.0 {
                        ray_arc(c, d, m, 0.0, 90.0, r, r)
                    } else {
                        ray_arc(c, d, m, 270.0, 360.0, r, r)
                    }
                }
            } else {
                or_front_border(c, d, &u)
            }
        })
    };
    let mut g = Geometry::new(c, border);
    text_anchors(&mut g, input);
    let west = if xor {
        let xb = 2.0 * h - 0.5 * lw;
        let x = c.x - 1.166666 * h - 0.866025 * 2.0 * h + xb + 0.5 * lw - ox - h / 3.0;
        if bb {
            x - 2.0 * 0.133974 * h
        } else {
            x
        }
    } else if bb {
        c.x - 1.16666 * h - ox
    } else {
        c.x - 1.16666 * h - ox + 0.267949 * h
    };
    for (name, p) in [
        ("east", east),
        ("north east", if bb { Point::new(east.x, c.y + hh) } else { or_corner(c, &u, 1.0) }),
        ("south east", if bb { Point::new(east.x, c.y - hh) } else { or_corner(c, &u, -1.0) }),
        ("north", Point::new(c.x, c.y + if bb { hh } else { 0.993043 * hh })),
        ("south", Point::new(c.x, c.y - if bb { hh } else { 0.993043 * hh })),
        ("north west", back(bb, 1.0)),
        ("south west", back(bb, -1.0)),
        ("west", Point::new(west, c.y)),
    ] {
        g.anchor_at(name, p);
    }
    side_anchors(&mut g, input, "east", east.x);
    side_anchors(&mut g, input, "west", back(bb, 1.0).x);
    let output = if inverted { Point::new(c.x - 0.166666 * h + 0.866025 * 2.0 * h + cm.outer_r + cm.r + ox, c.y) } else { east };
    g.anchor_at("output", output);
    input_anchors(&mut g, &cm, false, h, |_, y| or_input_x(&u, lw, y) + 0.5 * lw - ox - if xor { h / 3.0 } else { 0.0 });
    let (xc, yc) = (w - ox, hh - u.oy);
    let mut v = Vec::new();
    or_front(&mut v, c, xc, yc);
    let p = c.plus(Point::new(-1.16666 * xc, -yc));
    v.push(Segment::LineTo(p));
    if xor {
        let q = arc_to(&mut v, p, -22.0, 0.0, 1.166666 * 2.66666 * yc, 2.66666 * yc);
        arc_to(&mut v, q, 0.0, 22.0, 1.166666 * 2.66666 * yc, 2.66666 * yc);
    } else {
        let q = arc_to(&mut v, p, -30.0, 0.0, 2.0 * yc, 2.0 * yc);
        arc_to(&mut v, q, 0.0, 30.0, 2.0 * yc, 2.0 * yc);
    }
    v.push(Segment::Close);
    if inverted {
        v.extend(ellipse_path(c.plus(Point::new(-0.166666 * xc + 0.866025 * 2.0 * yc + cm.outer_r, 0.0)), cm.r, cm.r));
    }
    let back_shift = if xor { -0.333334 * h } else { 0.0 };
    input_circles(&mut v, &cm, yc, |y| or_input_x(&u, lw, y) + back_shift - cm.r);
    if xor {
        // the second line at the back, as short lines (so that it is not filled)
        let m = c.plus(Point::new(-3.232051 * h, 0.0));
        v.push(Segment::MoveTo(c.plus(Point::new(-1.5 * h, -h))));
        for k in 1..61 {
            let p = m.plus(polar(k as f64 - 30.0, 2.0 * h));
            v.push(Segment::LineTo(p));
            v.push(Segment::MoveTo(p));
        }
    }
    g.background(v);
    g
}

/// `not gate US` and `buffer gate US`: a triangle, with a circle at the not gate's tip.
pub fn not_gate_us(input: &mut Input, not: bool) -> Geometry {
    let cm = common(input, 1);
    let u = us_dimensions(input, &cm, false);
    let c = cm.c;
    let (h, w, hh, ox, oy) = (u.h, u.w, u.hh, u.ox, u.oy);
    let bb = input.keys.flag("tikra@gatebox");
    let tip = c.plus(u.tip);
    let far_tip = tip.plus(Point::new(2.350943 * ox, 0.0));
    let x_back = c.x - 0.833333 * h - ox;
    let nw = Point::new(x_back, c.y + 1.166666 * h + 1.565 * oy);
    let sw = Point::new(x_back, c.y - 1.166666 * h - 1.565 * oy);
    let east = if not { tip } else { far_tip };
    let output = if not { Point::new(c.x - 0.166666 * h + 0.866025 * 2.0 * h + cm.r + cm.outer_r + ox, c.y) } else { east };
    let in_x = c.x - 0.166666 * h - w + h / 3.0 - if cm.inputs.first() == Some(&true) { cm.r + cm.outer_r } else { 0.0 };
    let input_point = Point::new(in_x, c.y);
    let border = sampled(|d| {
        if d.y.abs() < 1e-12 {
            return if d.x > 0.0 { output } else { input_point };
        }
        let a = angle(c, c.plus(d));
        if a < angle(c, nw) {
            ray_line(c, d, nw, far_tip)
        } else if a < angle(c, sw) {
            ray_line(c, d, nw, sw)
        } else {
            ray_line(c, d, sw, far_tip)
        }
    });
    let mut g = Geometry::new(c, border);
    text_anchors(&mut g, input);
    let on_side = |dir: Point, corner: Point| ray_line(c, dir, corner, far_tip);
    for (name, p) in [
        ("east", east),
        ("output", output),
        ("north", if bb { Point::new(c.x, nw.y) } else { on_side(Point::new(0.0, 1.0), nw) }),
        ("south", if bb { Point::new(c.x, sw.y) } else { on_side(Point::new(0.0, -1.0), sw) }),
        ("north east", if bb { Point::new(tip.x, nw.y) } else { on_side(Point::new(1.0, 1.0), nw) }),
        ("south east", if bb { Point::new(tip.x, sw.y) } else { on_side(Point::new(1.0, -1.0), sw) }),
        ("north west", nw),
        ("south west", sw),
        ("west", Point::new(x_back, c.y)),
        ("input", input_point),
    ] {
        g.anchor_at(name, p);
    }
    side_anchors(&mut g, input, "east", east.x);
    side_anchors(&mut g, input, "west", x_back);
    let (xc, yc) = (w - ox, hh - oy);
    let mut v = vec![Segment::MoveTo(tip), Segment::LineTo(c.plus(Point::new(-0.833333 * xc, 1.166666 * yc))), Segment::LineTo(c.plus(Point::new(-0.833333 * xc, -1.166666 * yc)))];
    if not {
        // the not gate goes back to its tip instead of closing
        v.push(Segment::LineTo(tip));
        v.extend(ellipse_path(c.plus(Point::new(-0.166666 * xc + 0.866025 * 2.0 * yc + cm.outer_r, 0.0)), cm.r, cm.r));
    } else {
        v.push(Segment::Close);
    }
    if cm.inputs.first() == Some(&true) {
        v.extend(ellipse_path(c.plus(Point::new(-0.833333 * h - 0.5 * cm.lw - cm.r, 0.0)), cm.r, cm.r));
    }
    g.background(v);
    g
}

/// `and gate CDH` and `nand gate CDH`: the or gate's front on the and gate's square back.
pub fn and_gate_cdh(input: &mut Input, nand: bool) -> Geometry {
    let cm = common(input, 1024);
    let u = us_dimensions(input, &cm, false);
    let c = cm.c;
    let (h, w, hh, ox, oy) = (u.h, u.w, u.hh, u.ox, u.oy);
    let bb = input.keys.flag("tikra@gatebox");
    let east = c.plus(u.tip);
    let border = sampled(|d| if d.x < 0.0 { c.plus(super::border_rectangle(d, Point::new(0.166666 * h + w, hh))) } else { or_front_border(c, d, &u) });
    let mut g = Geometry::new(c, border);
    text_anchors(&mut g, input);
    let x_back = c.x - 1.16666 * h - ox;
    for (name, p) in [
        ("east", east),
        ("north east", if bb { Point::new(east.x, c.y + hh) } else { or_corner(c, &u, 1.0) }),
        ("south east", if bb { Point::new(east.x, c.y - hh) } else { or_corner(c, &u, -1.0) }),
        ("north", Point::new(c.x, c.y + if bb { hh } else { 0.993043 * hh })),
        ("south", Point::new(c.x, c.y - if bb { hh } else { 0.993043 * hh })),
        ("north west", Point::new(x_back, c.y + hh)),
        ("south west", Point::new(x_back, c.y - hh)),
        ("west", Point::new(x_back, c.y)),
    ] {
        g.anchor_at(name, p);
    }
    side_anchors(&mut g, input, "east", east.x);
    side_anchors(&mut g, input, "west", x_back);
    let output = if nand { Point::new(c.x - 0.166666 * h + 0.866025 * 2.0 * h + cm.r + cm.outer_r + ox, c.y) } else { east };
    g.anchor_at("output", output);
    input_anchors(&mut g, &cm, false, h, |_, _| -0.166666 * h - w);
    let (xc, yc) = (w - ox, hh - oy);
    let mut v = Vec::new();
    or_front(&mut v, c, xc, yc);
    v.push(Segment::LineTo(c.plus(Point::new(-1.16666 * xc, -yc))));
    v.push(Segment::LineTo(c.plus(Point::new(-1.16666 * xc, yc))));
    v.push(Segment::Close);
    if nand {
        v.extend(ellipse_path(c.plus(Point::new(-0.166666 * xc + 0.866025 * 2.0 * yc + cm.outer_r, 0.0)), cm.r, cm.r));
    }
    input_circles(&mut v, &cm, yc, |_| -1.16666 * h - 0.5 * cm.lw - cm.r);
    g.background(v);
    g
}

// ----------------------------------------------------------------------------------------- IEC

/// The IEC gates: `kind` is `and`, `nand`, `or`, `nor`, `xor`, `xnor`, `not` or `buffer`. A rectangle
/// around the text and the gate's symbol (`/pgf/and gate IEC symbol`…, placed by `/pgf/logic gate
/// IEC symbol align`), with a circle at the output of the inverting gates.
pub fn gate_iec(input: &mut Input, kind: &str) -> Geometry {
    let single = kind == "not" || kind == "buffer";
    let inverted = matches!(kind, "nand" | "nor" | "xnor" | "not");
    let cm = common(
        input,
        match kind {
            "xor" => 2,
            _ if single => 1,
            _ => 1024,
        },
    );
    let c = cm.c;
    // \pgf@lib@sh@logicgates@dimensions@IEC (the buffer gate measures the xor gate's symbol)
    let sized = if kind == "buffer" { "xor" } else { kind };
    let symbol_of = |input: &mut Input, k: &str| input.keys.text(&format!("/pgf/{k} gate IEC symbol"));
    let sized_symbol = symbol_of(input, sized);
    let sb = input.keys.text_box(&sized_symbol);
    let t = input.text();
    let mut x = (0.5 * t.wd).max(0.5 * sb.wd) + input.inner_xsep;
    let mut y = (0.5 * (t.ht + t.dp)).max(0.5 * (sb.ht + sb.dp)) + input.inner_ysep;
    x = x.max(0.5 * input.minimum_width);
    y = y.max(0.5 * input.minimum_height);
    let n1 = (cm.inputs.len() + 1) as f64;
    let yb = (2.0 * cm.r + cm.lw).max(cm.sep) * n1;
    y = y.max(0.5 * yb);
    let (ihw, ihh) = (x - input.inner_xsep, y - input.inner_ysep);
    let (ox, oy) = (input.outer_xsep, input.outer_ysep);
    let (w, hh) = (x + ox, y + oy);
    let mut g = Geometry::new(c, Border::Rectangle(Point::new(w, hh)));
    text_anchors(&mut g, input);
    let p = |dx: f64, dy: f64| c.plus(Point::new(dx, dy));
    for (name, q) in [
        ("north", p(0.0, hh)),
        ("south", p(0.0, -hh)),
        ("east", p(w, 0.0)),
        ("west", p(-w, 0.0)),
        ("north east", p(w, hh)),
        ("north west", p(-w, hh)),
        ("south east", p(w, -hh)),
        ("south west", p(-w, -hh)),
    ] {
        g.anchor_at(name, q);
    }
    side_anchors(&mut g, input, "east", c.x + w);
    side_anchors(&mut g, input, "west", c.x - w);
    g.anchor_at("output", p(w + if inverted { cm.r + cm.outer_r } else { 0.0 }, 0.0));
    input_anchors(&mut g, &cm, single, hh, |_, _| -w);
    // the background: the rectangle (in by the outer x sep, also vertically), the inputs' circles
    let (xc, yc) = (w - ox, hh - ox);
    let mut v = vec![Segment::MoveTo(p(-xc, -yc)), Segment::LineTo(p(xc, -yc)), Segment::LineTo(p(xc, yc)), Segment::LineTo(p(-xc, yc)), Segment::Close];
    input_circles(&mut v, &cm, hh, |_| -w - cm.r);
    if inverted {
        v.extend(ellipse_path(p(w + cm.r, 0.0), cm.r, cm.r));
    }
    g.background(v);
    // the symbol, by \pgftext at the center moved to the side of the alignment
    let symbol = symbol_of(input, kind);
    let b = input.keys.text_box(&symbol);
    let align = input.keys.text("/pgf/logic gate IEC symbol align");
    // each word moves the point to that side and puts that side of the text there, as
    // `\pgftext[bottom,right]` (`top` and `bottom` the text's top and bottom, not its baseline)
    let (mut x, mut y) = (-b.wd / 2.0, -(b.ht - b.dp) / 2.0);
    for word in align.split(',').map(str::trim) {
        match word {
            "top" => y = ihh - b.ht,
            "bottom" => y = -ihh + b.dp,
            "left" => x = -ihw,
            "right" => x = ihw - b.wd,
            _ => {}
        }
    }
    let at = p(x, y);
    let color = input.keys.text("/pgf/logic gate IEC symbol color");
    g.texts.push(ShapeText { tex: symbol, at, color: Some(color).filter(|c| !c.trim().is_empty()), anchor: None, font: None, turn: 0.0, upright: false });
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_short_and_long() {
        assert_eq!(parse_inputs("nin", 1024), vec![false, true, false]);
        assert_eq!(parse_inputs("inverted, normal, inverted", 1024), vec![true, false, true]);
        assert_eq!(parse_inputs("normal,normal", 1024), vec![false, false]);
        assert_eq!(parse_inputs("iii", 2), vec![true, true]);
    }

    #[test]
    fn a_ray_meets_an_arc() {
        let p = ray_arc(Point::new(0.0, 0.0), Point::new(1.0, 0.0), Point::new(0.0, 0.0), -90.0, 90.0, 2.0, 1.0);
        assert!((p.x - 2.0).abs() < 1e-12 && p.y.abs() < 1e-12);
    }
}
