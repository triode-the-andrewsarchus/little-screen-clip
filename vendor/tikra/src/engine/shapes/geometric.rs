//! The shapes of `pgflibraryshapes.geometric.code.tex` (but the ellipse and diamond, in `basic`).
//!
//! Most compute a polygon from the text box and their keys, rotate it by `shape border rotate`
//! (rounded to a multiple of 90° unless `shape border uses incircle`), and find their border points
//! where a ray from the center (or from the base or mid point, for the `base`/`mid` anchors) meets
//! the polygon moved out by the outer sep, as pgf does side by side.

use std::f64::consts::SQRT_2;

use crate::scene::{Point, Segment, Transform};

use super::{arc_points, arc_to, cos, cot, miter_offset, polar, polygon_path, ray, rotate_around, sin, tan, Border, Geometry, Input, Layer, Usage};

/// The eight compass anchors: the border points toward them from the center.
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

/// `center`, `mid` and `base`, at the text's center, half an ex up and baseline.
fn text_anchors(g: &mut Geometry, input: &Input) {
    g.anchor_at("center", input.center_point());
    g.anchor_at("mid", input.mid_point());
    g.anchor_at("base", input.base_point());
}

/// `mid west`, `mid east`, `base west`, `base east`: from the mid and base points, sideways to the
/// outline.
fn side_anchors(g: &mut Geometry, input: &Input, outline: &[Point]) {
    let (m, b) = (input.mid_point(), input.base_point());
    g.anchor_at("mid west", ray(m, Point::new(-1.0, 0.0), outline));
    g.anchor_at("mid east", ray(m, Point::new(1.0, 0.0), outline));
    g.anchor_at("base west", ray(b, Point::new(-1.0, 0.0), outline));
    g.anchor_at("base east", ray(b, Point::new(1.0, 0.0), outline));
}

fn rotated(points: &[Point], c: Point, angle: f64) -> Vec<Point> {
    points.iter().map(|p| rotate_around(*p, c, angle)).collect()
}

/// Width and height swapped when the border is rotated by 90° or 270°.
fn swap_for(rotate: f64, x: f64, y: f64) -> (f64, f64) {
    if rotate == 90.0 || rotate == 270.0 {
        (y, x)
    } else {
        (x, y)
    }
}

pub fn star(input: &mut Input) -> Geometry {
    let points = input.number("/pgf/star points", 5.0).trunc().max(2.0);
    let step = 180.0 / points;
    let uses_ratio = input.keys.flag("tikra@sh@starratio");
    let ratio = input.number("/pgf/star point ratio", 1.5);
    let height = input.length("/pgf/star point height", 0.5);
    let (x, y) = input.half_with_inner();
    let mut inner = SQRT_2 * x.max(y);
    let mut outer = if uses_ratio { ratio * inner } else { inner + height };
    let natural = outer;
    let min = input.minimum_width.max(input.minimum_height);
    if outer < min / 2.0 {
        outer = min / 2.0;
    }
    if outer > natural {
        inner = if uses_ratio { outer / ratio } else { outer - height };
    }
    let start = 90.0 + input.number("/pgf/shape border rotate", 0.0);
    let c = input.center_point();
    let n = (2.0 * points) as usize;
    let vertex = |k: usize, r_out: f64, r_in: f64| c.plus(polar(start + k as f64 * step, if k.is_multiple_of(2) { r_out } else { r_in }));
    let shape: Vec<Point> = (0..n).map(|k| vertex(k, outer, inner)).collect();
    // the anchor radii: the corners moved out by the miter length of the outer sep
    let border = miter_offset(&shape, input.outer_sep());
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

pub fn regular_polygon(input: &mut Input) -> Geometry {
    let sides = input.number("/pgf/regular polygon sides", 5.0).trunc().max(3.0);
    let step = 360.0 / sides;
    let (x, y) = input.half_with_inner();
    // the incircle around the text, then the polygon around it
    let incircle = SQRT_2 * x.max(y);
    let min = input.minimum_width.max(input.minimum_height);
    let radius = (incircle / cos(180.0 / sides)).max(min / 2.0);
    let anchor_radius = radius + input.outer_sep() / sin(90.0 - 180.0 / sides);
    let rotate = input.number("/pgf/shape border rotate", 0.0);
    let start = if (sides as i64) % 2 == 1 { 90.0 } else { 90.0 - step / 2.0 } + rotate;
    let c = input.center_point();
    let n = sides as usize;
    let corner = |k: usize, r: f64| c.plus(polar(start + k as f64 * step, r));
    let shape: Vec<Point> = (0..n).map(|k| corner(k, radius)).collect();
    let border: Vec<Point> = (0..n).map(|k| corner(k, anchor_radius)).collect();
    let mut g = Geometry::new(c, Border::Outline(border.clone()));
    text_anchors(&mut g, input);
    compass(&mut g);
    for k in 0..n {
        g.anchor_at(&format!("corner {}", k + 1), border[k]);
        g.anchor_at(&format!("side {}", k + 1), border[k].lerp(border[(k + 1) % n], 0.5));
    }
    g.background(polygon_path(&shape));
    g.roundable = true;
    g
}

/// `\pgfmathmod` of an angle into [0, 360).
fn angle_mod(a: f64) -> f64 {
    let m = a % 360.0;
    if m < 0.0 {
        m + 360.0
    } else {
        m
    }
}

pub fn trapezium(input: &mut Input) -> Geometry {
    let (mut x, mut y) = input.half_with_inner();
    let (rotate, incircle) = input.border_rotation();
    if incircle {
        x = SQRT_2 * x.max(y);
        y = x;
    } else {
        (x, y) = swap_for(rotate, x, y);
    }
    let extension = |angle: f64, y: f64| if angle == 0.0 { 0.0 } else { 2.0 * y * cot(angle) };
    let left = angle_mod(input.number("/pgf/trapezium left angle", 60.0));
    let right = angle_mod(input.number("/pgf/trapezium right angle", 60.0));
    let mut xa = extension(left, y);
    let mut xb = extension(right, y);
    let stretches = input.keys.flag("tikra@sh@trapstretch");
    let stretches_body = input.keys.flag("tikra@sh@trapbody");
    let (min_w, min_h) = (input.minimum_width, input.minimum_height);
    if y < min_h / 2.0 {
        if stretches || stretches_body {
            y = min_h / 2.0;
        } else {
            let s = (min_h / 2.0) / y;
            y = min_h / 2.0;
            x *= s;
            xa *= s;
            xb *= s;
        }
    }
    let width = 2.0 * x + xa.abs() + xb.abs();
    if width < min_w {
        if stretches_body {
            x += (min_w - width) / 2.0;
        } else {
            let s = min_w / width;
            x *= s;
            xa *= s;
            xb *= s;
            if !stretches {
                y *= s;
            }
        }
    }
    let c = input.center_point();
    let lower_left = c.plus(Point::new(-x - xa.max(0.0), -y));
    let upper_left = c.plus(Point::new(-x + xa.min(0.0), y));
    let upper_right = c.plus(Point::new(x - xb.min(0.0), y));
    let lower_right = c.plus(Point::new(x + xb.max(0.0), -y));
    let shape = [lower_left, upper_left, upper_right, lower_right];
    let border = rotated(&miter_offset(&shape, input.outer_sep()), c, rotate);
    let shape = rotated(&shape, c, rotate);
    let mut g = Geometry::new(c, Border::Outline(border.clone()));
    text_anchors(&mut g, input);
    side_anchors(&mut g, input, &border);
    compass(&mut g);
    let [ll, ul, ur, lr] = [border[0], border[1], border[2], border[3]];
    for (name, p) in [
        ("bottom left corner", ll),
        ("top left corner", ul),
        ("top right corner", ur),
        ("bottom right corner", lr),
        ("left side", ll.lerp(ul, 0.5)),
        ("right side", lr.lerp(ur, 0.5)),
        ("top side", ul.lerp(ur, 0.5)),
        ("bottom side", ll.lerp(lr, 0.5)),
    ] {
        g.anchor_at(name, p);
    }
    g.background(polygon_path(&shape));
    g.roundable = true;
    g
}

pub fn semicircle(input: &mut Input) -> Geometry {
    let o = input.outer_sep();
    let (mut x, mut y) = input.half_with_inner();
    let (rotate, incircle) = input.border_rotation();
    let half_height;
    if incircle {
        x = SQRT_2 * x.max(y);
        half_height = x;
        x *= 2.0;
    } else {
        (x, y) = swap_for(rotate, x, y);
        x = x.hypot(2.0 * y);
        half_height = y;
    }
    let default_radius = x;
    let radius = x.max(input.minimum_width / 2.0).max(input.minimum_height);
    let c = input.center_point();
    // moved down so that the text looks centered when the shape is enlarged (pgf's .4)
    let arc_center = c.plus(Point::new(0.0, -0.4 * (radius - default_radius) - half_height));
    let r = radius + o;
    let start_border = arc_center.plus(Point::new(r, 0.0));
    let end_border = arc_center.plus(Point::new(-r, 0.0));
    let start_corner = start_border.minus(Point::new(0.0, o));
    let end_corner = end_border.minus(Point::new(0.0, o));
    let rot = |p: Point| rotate_around(p, c, rotate);
    let arc_center = rot(arc_center);
    let mut outline = arc_points(arc_center, r, r, rotate, rotate + 180.0);
    outline.push(rot(end_corner));
    outline.push(rot(start_corner));
    let mut g = Geometry::new(c, Border::Outline(outline.clone()));
    text_anchors(&mut g, input);
    side_anchors(&mut g, input, &outline);
    compass(&mut g);
    g.anchor_at("apex", arc_center.plus(polar(rotate + 90.0, r)));
    g.anchor_at("arc start", rot(start_corner));
    g.anchor_at("arc end", rot(end_corner));
    g.anchor_at("chord center", rot(start_corner).lerp(rot(end_corner), 0.5));
    let mut v = vec![Segment::MoveTo(arc_center.plus(polar(rotate, radius)))];
    super::arc_to_rotated(&mut v, arc_center.plus(polar(rotate, radius)), 0.0, 180.0, radius, radius, rotate);
    v.push(Segment::Close);
    g.background(v);
    g
}

pub fn isosceles_triangle(input: &mut Input) -> Geometry {
    let mut half = input.number("/pgf/isosceles triangle apex angle", 45.0) / 2.0;
    let (tan_half, cot_half) = (tan(half), cot(half));
    let (mut x, mut y) = input.half_with_inner();
    let (rotate, incircle) = input.border_rotation();
    let (mut xa, mut ya);
    if incircle {
        x = SQRT_2 * x.max(y);
        y = x;
        xa = x + x / sin(half);
        ya = tan_half * xa;
    } else {
        (x, y) = swap_for(rotate, x, y);
        xa = 2.0 * x;
        ya = tan_half * xa + y;
        xa += cot_half * y;
    }
    if xa == 0.0 {
        xa = input.minimum_height;
    }
    let stretches = input.keys.flag("tikra@sh@isostretch");
    if ya < input.minimum_width / 2.0 {
        ya = input.minimum_width / 2.0;
        if stretches {
            half = (ya / xa).atan().to_degrees();
        } else {
            xa = cot_half * ya;
        }
    }
    if xa < input.minimum_height {
        xa = input.minimum_height;
        if stretches {
            half = (ya / xa).atan().to_degrees();
        } else {
            ya = tan_half * xa;
        }
    }
    // the distance from the center of the text to the base of the triangle
    let offset = if incircle { xa * sin(half) / (1.0 + sin(half)) } else { ((ya - y) * cos(half) - 2.0 * sin(half) * x) / (1.0 + sin(half)) + x };
    let apex = Point::new(xa - offset, 0.0);
    let lower_left = Point::new(-offset, ya);
    let o = input.outer_sep();
    let apex_anchor = apex.plus(Point::new(o / sin(half), 0.0));
    let lower_left_anchor = lower_left.plus(Point::new(-o, cot((90.0 - half) / 2.0) * o));
    let lower_right_anchor = Point::new(lower_left_anchor.x, -lower_left_anchor.y);
    let c = input.center_point();
    let place = |p: Point| rotate_around(c.plus(p), c, rotate);
    let (a, l, r) = (place(apex_anchor), place(lower_left_anchor), place(lower_right_anchor));
    let outline = vec![a, l, r];
    let mut g = Geometry::new(c, Border::Outline(outline.clone()));
    text_anchors(&mut g, input);
    side_anchors(&mut g, input, &outline);
    compass(&mut g);
    for (name, p) in [("apex", a), ("left corner", l), ("right corner", r), ("left side", l.lerp(a, 0.5)), ("right side", r.lerp(a, 0.5)), ("lower side", l.lerp(r, 0.5))] {
        g.anchor_at(name, p);
    }
    g.background(polygon_path(&[place(apex), place(lower_left), place(Point::new(lower_left.x, -lower_left.y))]));
    g.roundable = true;
    g
}

pub fn kite(input: &mut Input) -> Geometry {
    let hu = input.number("/pgf/kite upper vertex angle", 120.0) / 2.0;
    let hl = input.number("/pgf/kite lower vertex angle", 60.0) / 2.0;
    let (mut x, mut y) = input.half_with_inner();
    let (rotate, incircle) = input.border_rotation();
    let (mut xa, mut ya, mut yb, delta_y);
    if incircle {
        let r = SQRT_2 * x.max(y);
        ya = r / sin(hu);
        yb = r / sin(hl);
        let yc = r / sin(hu) - r / sin(hu + hl) * (sin(hu) + sin(hl)) * cos(hu) / sin(hu);
        delta_y = yc;
        ya -= yc;
        yb += yc;
        xa = tan(hu) * ya;
    } else {
        x *= 2.0;
        y *= 2.0;
        (x, y) = swap_for(rotate, x, y);
        // the text reaches ya into the upper triangle and yb into the lower one
        ya = y / sin(hu + hl) * cos(hu) * sin(hl);
        yb = y - ya;
        delta_y = y / 2.0 - ya;
        xa = x / 2.0 + tan(hu) * ya;
        ya += cot(hu) * x / 2.0;
        yb += cot(hl) * x / 2.0;
    }
    let total = ya + yb;
    if total < input.minimum_height {
        let s = input.minimum_height / total;
        xa *= s;
        ya *= s;
        yb *= s;
    }
    if 2.0 * xa < input.minimum_width {
        let s = input.minimum_width / (2.0 * xa);
        xa *= s;
        ya *= s;
        yb *= s;
    }
    let c = input.center_point();
    let top = c.plus(Point::new(0.0, delta_y + ya));
    let bottom = c.plus(Point::new(0.0, delta_y - yb));
    let left = c.plus(Point::new(-xa, delta_y));
    let right = c.plus(Point::new(xa, delta_y));
    let shape = [top, left, bottom, right];
    let border = rotated(&miter_offset(&shape, input.outer_sep()), c, rotate);
    let shape = rotated(&shape, c, rotate);
    let mut g = Geometry::new(c, Border::Outline(border.clone()));
    text_anchors(&mut g, input);
    side_anchors(&mut g, input, &border);
    compass(&mut g);
    let [t, l, b, r] = [border[0], border[1], border[2], border[3]];
    for (name, p) in [
        ("upper vertex", t),
        ("lower vertex", b),
        ("left vertex", l),
        ("right vertex", r),
        ("upper left side", t.lerp(l, 0.5)),
        ("lower left side", b.lerp(l, 0.5)),
        ("upper right side", t.lerp(r, 0.5)),
        ("lower right side", b.lerp(r, 0.5)),
    ] {
        g.anchor_at(name, p);
    }
    g.background(polygon_path(&shape));
    g.roundable = true;
    g
}

pub fn dart(input: &mut Input) -> Geometry {
    let hp = input.number("/pgf/dart tip angle", 45.0) / 2.0;
    let ht = input.number("/pgf/dart tail angle", 135.0) / 2.0;
    let (mut x, mut y) = input.half_with_inner();
    // an empty box (decorations' shape backgrounds): a tiny one, scaled up to the minimum size below
    if x.max(y) < 1e-9 {
        (x, y) = (1e-6, 1e-6);
    }
    let (rotate, incircle) = input.border_rotation();
    let mut xa;
    if incircle {
        x = SQRT_2 * x.max(y);
        xa = cot(hp) * x + x;
    } else {
        (x, y) = swap_for(rotate, x, y);
        xa = cot(hp) * y + 2.0 * x;
    }
    // the half separation of the tails (pgf's formula, with the cosine of the half tip angle)
    let mut ya = xa / sin(ht - hp) * sin(hp) * cos(hp);
    let xb = cot(hp) * ya;
    let mut xc = xb - xa;
    if xb < input.minimum_height {
        let s = input.minimum_height / xb;
        xa *= s;
        xc *= s;
        ya *= s;
        x *= s;
    }
    if ya < input.minimum_width / 2.0 {
        let s = (input.minimum_width / 2.0) / ya;
        ya = input.minimum_width / 2.0;
        xa *= s;
        xc *= s;
        x *= s;
    }
    let c = input.center_point();
    let tip = c.plus(Point::new(xa - x, 0.0));
    let tail_center = c.plus(Point::new(-x, 0.0));
    let left_tail = c.plus(Point::new(-x - xc, ya));
    let right_tail = c.plus(Point::new(-x - xc, -ya));
    let shape = [tip, left_tail, tail_center, right_tail];
    let border = rotated(&miter_offset(&shape, input.outer_sep()), c, rotate);
    let shape = rotated(&shape, c, rotate);
    let mut g = Geometry::new(c, Border::Outline(border.clone()));
    text_anchors(&mut g, input);
    side_anchors(&mut g, input, &border);
    compass(&mut g);
    let [t, l, m, r] = [border[0], border[1], border[2], border[3]];
    for (name, p) in [("tip", t), ("left tail", l), ("right tail", r), ("tail center", m), ("left side", t.lerp(l, 0.5)), ("right side", t.lerp(r, 0.5))] {
        g.anchor_at(name, p);
    }
    g.background(polygon_path(&shape));
    g.roundable = true;
    g
}

pub fn circular_sector(input: &mut Input) -> Geometry {
    let o = input.outer_sep();
    let half = angle_mod(input.number("/pgf/circular sector angle", 60.0)) / 2.0;
    let center_miter = o / sin(half);
    let (start, end) = (180.0 - half, 180.0 + half);
    let (mut x, mut y) = input.half_with_inner();
    let (rotate, incircle) = input.border_rotation();
    let (mut radius, mut offset);
    if incircle {
        x = SQRT_2 * x.max(y);
        offset = x / sin(half);
        radius = offset + x;
    } else {
        (x, y) = swap_for(rotate, x, y);
        offset = y / sin(half) * cos(half) + x;
        radius = (offset + x).hypot(y);
    }
    let reach = (offset / cos(half)).abs();
    if reach < input.minimum_width / 2.0 {
        let s = (input.minimum_width / 2.0) / reach;
        radius *= s;
        offset *= s;
    }
    if radius < input.minimum_height {
        let s = input.minimum_height / radius;
        radius = input.minimum_height;
        offset *= s;
    }
    let border_radius = radius + o;
    let corner_radius = border_radius + o / sin(half) * cos(half);
    let c = input.center_point();
    let sector_center = c.plus(Point::new(offset, 0.0));
    let rot = |p: Point| rotate_around(p, c, rotate);
    let center_border = sector_center.plus(Point::new(center_miter, 0.0));
    let start_corner = center_border.plus(polar(start, corner_radius));
    let end_corner = center_border.plus(polar(end, corner_radius));
    let mut outline = vec![rot(center_border), rot(start_corner)];
    outline.extend(arc_points(rot(sector_center), border_radius, border_radius, start + rotate, end + rotate));
    outline.push(rot(end_corner));
    let mut g = Geometry::new(c, Border::Outline(outline));
    text_anchors(&mut g, input);
    compass(&mut g);
    g.anchor_at("arc start", rot(start_corner));
    g.anchor_at("arc end", rot(end_corner));
    g.anchor_at("sector center", rot(center_border));
    let arc_center = g.border_point(polar(rotate + 180.0, corner_radius));
    g.anchor_at("arc center", arc_center);
    let sc = rot(sector_center);
    let arc_start = sc.plus(polar(start + rotate, radius));
    let mut v = vec![Segment::MoveTo(sc), Segment::LineTo(arc_start)];
    arc_to(&mut v, arc_start, start + rotate, end + rotate, radius, radius);
    v.push(Segment::Close);
    g.background(v);
    g
}

pub fn cylinder(input: &mut Input) -> Geometry {
    let (mut x, mut y) = input.half_with_inner();
    let mut inner_y = input.inner_ysep;
    let (rotate, incircle) = input.border_rotation();
    if incircle {
        x = SQRT_2 * x.max(y);
        y = x;
    } else if rotate == 90.0 || rotate == 270.0 {
        (x, y) = (y, x);
        inner_y = input.inner_xsep;
    }
    let (mut xa, mut ya) = (x, y);
    // the radii of the ends
    let rx = input.aspect * ya;
    let mut ry = ya;
    if ry < input.minimum_width / 2.0 {
        ry = input.minimum_width / 2.0;
        ya = ry;
    }
    // how far the text can reach into the bottom end
    let xb = ((ry - inner_y) / ry).clamp(-1.0, 1.0).asin().cos() * rx;
    let lw = input.line_width;
    let length = 0.5 * lw + 2.0 * xa + 3.0 * rx - xb;
    if length < input.minimum_height {
        xa += (input.minimum_height - length) / 2.0;
    }
    let o = input.outer_sep();
    let (xr, yr) = (rx + o, ry + o);
    let before_top = Point::new(xa + rx + 0.5 * lw, ya);
    let after_bottom = Point::new(-xa + xb, ya);
    let oy = input.outer_ysep;
    let before_top_anchor = before_top.plus(Point::new(0.0, oy));
    let after_bottom_anchor = after_bottom.plus(Point::new(0.0, oy));
    let c = input.center_point();
    let place = |p: Point| rotate_around(c.plus(p), c, rotate);
    let mut outline: Vec<Point> = arc_points(Point::new(before_top.x, 0.0), xr, yr, -90.0, 90.0).into_iter().map(place).collect();
    outline.extend(arc_points(Point::new(after_bottom.x, 0.0), xr, yr, 90.0, 270.0).into_iter().map(place));
    let mut g = Geometry::new(c, Border::Outline(outline.clone()));
    text_anchors(&mut g, input);
    side_anchors(&mut g, input, &outline);
    compass(&mut g);
    for (name, p) in [
        ("shape center", Point::new((rx + 0.5 * lw + xb) / 2.0, 0.0)),
        ("before top", before_top_anchor),
        ("top", Point::new(before_top.x + xr, 0.0)),
        ("after top", Point::new(before_top_anchor.x, -before_top_anchor.y)),
        ("before bottom", Point::new(after_bottom_anchor.x, -after_bottom_anchor.y)),
        ("bottom", Point::new(after_bottom.x - xr, 0.0)),
        ("after bottom", after_bottom_anchor),
    ] {
        g.anchor_at(name, place(p));
    }
    let t = Transform::translation(c.x, c.y).compose(&Transform::rotation(rotate));
    let local = |f: &dyn Fn(&mut Vec<Segment>)| -> Vec<Segment> {
        let mut v = Vec::new();
        f(&mut v);
        crate::canvas::transform_segments(&v, &t)
    };
    if input.keys.flag("pgfcylinderusescustomfill") {
        let body = local(&|v| {
            v.push(Segment::MoveTo(after_bottom));
            arc_to(v, after_bottom, 90.0, 270.0, rx, ry);
            let q = Point::new(before_top.x, -before_top.y);
            v.push(Segment::LineTo(q));
            arc_to(v, q, 270.0, 90.0, rx, ry);
            v.push(Segment::Close);
        });
        let body_fill = input.keys.text("/pgf/cylinder body fill");
        g.path(Layer::BehindBackground, body, Usage::Fill(body_fill));
        let end = local(&|v| {
            v.push(Segment::MoveTo(before_top));
            arc_to(v, before_top, 90.0, -270.0, rx, ry);
            v.push(Segment::Close);
        });
        let end_fill = input.keys.text("/pgf/cylinder end fill");
        g.path(Layer::BehindBackground, end, Usage::Fill(end_fill));
    }
    g.background(local(&|v| {
        v.push(Segment::MoveTo(after_bottom));
        arc_to(v, after_bottom, 90.0, 270.0, rx, ry);
        let q = Point::new(before_top.x, -before_top.y);
        v.push(Segment::LineTo(q));
        arc_to(v, q, -90.0, 90.0, rx, ry);
        v.push(Segment::Close);
        v.push(Segment::MoveTo(before_top));
        arc_to(v, before_top, 90.0, 270.0, rx, ry);
    }));
    g
}
