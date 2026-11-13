//! The shapes of `pgflibraryshapes.multipart.code.tex`: circle split, circle solidus, ellipse
//! split, diamond split and rectangle split, whose texts are in several node parts (`\nodepart`).

use std::f64::consts::{FRAC_1_SQRT_2, SQRT_2};

use crate::scene::{Point, Segment};

use super::basic::ellipse_path;
use super::{Border, BoxSize, Geometry, Input, Layer, Usage};

/// The anchors of a circle or ellipse of radii `r` around `c`.
fn round_anchors(g: &mut Geometry, input: &Input, c: Point, r: Point) {
    let k = FRAC_1_SQRT_2;
    let m = 0.5 * input.ex;
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

/// A circle of radius `r` (outer sep included) around `c`, with a text and a lower part.
fn split_circle(input: &Input, c: Point, r: f64, lower: Point) -> Geometry {
    let mut g = Geometry::new(c, Border::Ellipse(Point::new(r, r)));
    round_anchors(&mut g, input, c, Point::new(r, r));
    g.anchor_at("lower", lower);
    g.parts.push(("lower".into(), lower));
    let rr = r - input.outer_sep();
    g.background(ellipse_path(c, rr, rr));
    g
}

pub fn circle_split(input: &mut Input) -> Geometry {
    let (t, l) = (input.text(), input.part("lower"));
    let lw = input.line_width;
    let c = Point::new(t.wd / 2.0, -input.inner_ysep - t.dp - lw / 2.0);
    let lower = Point::new(t.wd / 2.0 - l.wd / 2.0, -2.0 * input.inner_ysep - l.ht - lw / 2.0 - t.dp - lw / 2.0);
    // the taller part, twice, and the wider one
    let ya = 2.0 * ((t.ht + t.dp) / 2.0).max((l.ht + l.dp) / 2.0) + lw / 2.0 + 2.0 * input.inner_ysep;
    let xa = (t.wd / 2.0).max(l.wd / 2.0) + input.inner_xsep;
    let r = xa.hypot(ya).max(input.minimum_width / 2.0).max(input.minimum_height / 2.0) + input.outer_sep();
    let mut g = split_circle(input, c, r, lower);
    let d = r - input.outer_sep() - lw / 2.0;
    g.path(Layer::BeforeBackground, vec![Segment::MoveTo(c.plus(Point::new(-d, 0.0))), Segment::LineTo(c.plus(Point::new(d, 0.0)))], Usage::Stroke);
    g
}

pub fn circle_solidus(input: &mut Input) -> Geometry {
    let (t, l) = (input.text(), input.part("lower"));
    let lw = input.line_width;
    let (ix, iy) = (input.inner_xsep, input.inner_ysep);
    let x0 = t.wd / 2.0 + t.dp / 2.0 + t.ht / 2.0;
    let y0 = -x0 + t.ht;
    let c = Point::new(x0 + 0.3536 * lw + ix, y0 - 0.3536 * lw - iy);
    let lower = Point::new(x0 + (l.ht + l.dp) / 2.0 - l.wd / 2.0 + FRAC_1_SQRT_2 * lw + 2.0 * ix, y0 - l.ht / 2.0 + l.dp / 2.0 - l.wd / 2.0 - FRAC_1_SQRT_2 * lw - 2.0 * iy);
    let r = (FRAC_1_SQRT_2 * (t.ht + t.dp + t.wd).max(l.ht + l.dp + l.wd) + lw / 2.0 + (2.0 * ix).hypot(2.0 * iy)).max(input.minimum_width / 2.0).max(input.minimum_height / 2.0)
        + input.outer_sep();
    let mut g = split_circle(input, c, r, lower);
    let d = 0.437 * (r - input.outer_sep() - lw / 2.0);
    g.path(Layer::BeforeBackground, vec![Segment::MoveTo(c.plus(Point::new(-d, -d))), Segment::LineTo(c.plus(Point::new(d, d)))], Usage::Stroke);
    g
}

pub fn ellipse_split(input: &mut Input) -> Geometry {
    let (t, l) = (input.text(), input.part("lower"));
    let lw = input.line_width;
    let (ix, iy) = (input.inner_xsep, 2.0 * input.inner_ysep + lw / 2.0);
    let rx = (SQRT_2 * (t.wd / 2.0 + ix).max(l.wd / 2.0 + ix)).max(input.minimum_width / 2.0) + input.outer_xsep;
    let ry = (SQRT_2 * (t.ht + t.dp + iy).max(l.ht + l.dp + iy)).max(input.minimum_height / 2.0) + input.outer_ysep;
    let c = Point::new(t.wd / 2.0, -input.inner_ysep - t.dp - lw / 2.0);
    let lower = Point::new(t.wd / 2.0 - l.wd / 2.0, -2.0 * input.inner_ysep - lw - t.dp - l.ht);
    let mut g = Geometry::new(c, Border::Ellipse(Point::new(rx, ry)));
    round_anchors(&mut g, input, c, Point::new(rx, ry));
    g.anchor_at("lower", lower);
    g.parts.push(("lower".into(), lower));
    let (ax, ay) = (rx - input.outer_xsep, ry - input.outer_ysep);
    let mut v = ellipse_path(c, ax, ay);
    v.push(Segment::MoveTo(c.plus(Point::new(-ax, 0.0))));
    v.push(Segment::LineTo(c.plus(Point::new(ax, 0.0))));
    g.background(v);
    g
}

pub fn diamond_split(input: &mut Input) -> Geometry {
    let (t, l) = (input.text(), input.part("lower"));
    let (ix, iy) = (input.inner_xsep, input.inner_ysep);
    // the larger of the two parts (pgf adds no inner sep to the text part)
    let xa = t.wd.max(l.wd + ix);
    let ya = (t.ht + t.dp).max(l.ht + l.dp + iy);
    let aspect = if input.aspect.abs() > 1e-9 { input.aspect } else { 1.0 };
    let x = (xa + aspect * ya).max(input.minimum_width / 2.0) + input.outer_xsep;
    let y = (xa / aspect + ya).max(input.minimum_height / 2.0) + input.outer_ysep;
    // pgf's diamond is around the origin, with the text part's origin at its `text` anchor
    let text = Point::new(-t.wd / 2.0, 0.25 * (t.ht + t.dp) + input.outer_ysep);
    let c = Point::new(0.0, 0.0).minus(text);
    let p = |dx: f64, dy: f64| c.plus(Point::new(dx, dy));
    let lower = p(-l.wd / 2.0, -1.25 * (l.ht + l.dp) - input.outer_ysep);
    let mut g = Geometry::new(c, Border::Outline(vec![p(x, 0.0), p(0.0, y), p(-x, 0.0), p(0.0, -y)]));
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
        ("lower", lower),
    ] {
        g.anchor_at(name, q);
    }
    g.parts.push(("lower".into(), lower));
    let (xc, yc) = (x - SQRT_2 * input.outer_xsep, y - SQRT_2 * input.outer_ysep);
    // the split, then the diamond, in one path
    let ox = input.outer_xsep;
    g.background(vec![
        Segment::MoveTo(p(-x + ox, 0.0)),
        Segment::LineTo(p(x - ox, 0.0)),
        Segment::MoveTo(p(xc, 0.0)),
        Segment::LineTo(p(0.0, yc)),
        Segment::LineTo(p(-xc, 0.0)),
        Segment::LineTo(p(0.0, -yc)),
        Segment::Close,
    ]);
    g
}

/// The name of the k-th part of a rectangle split (1 is `text`).
pub fn part_name(k: usize) -> &'static str {
    const NAMES: [&str; 20] = [
        "text",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
        "twenty",
    ];
    NAMES[(k - 1).min(19)]
}

/// The k-th part's name in anchors (`one`, `two`, …).
fn anchor_name(k: usize) -> &'static str {
    if k == 1 {
        "one"
    } else {
        part_name(k)
    }
}

/// A list of values for the parts (`left, right`): the k-th, or the last one for the rest.
fn per_part(list: &str, n: usize) -> Vec<String> {
    let items: Vec<String> = list.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    (0..n).map(|k| items.get(k).or(items.last()).cloned().unwrap_or_default()).collect()
}

pub fn rectangle_split(input: &mut Input) -> Geometry {
    let n = input.number("/pgf/rectangle split parts", 4.0).trunc().clamp(1.0, 20.0) as usize;
    let horizontal = input.keys.flag("tikra@sh@splithoriz");
    let ignore_empty = input.keys.flag("tikra@sh@splitskipempty");
    let draw_splits = input.keys.flag("tikra@sh@splitlines");
    let ex = input.ex;
    let empty = BoxSize {
        wd: input.length("/pgf/rectangle split empty part width", ex),
        ht: input.length("/pgf/rectangle split empty part height", ex),
        dp: input.length("/pgf/rectangle split empty part depth", 0.0),
    };
    let (ix, iy, ox, oy, lw) = (input.inner_xsep, input.inner_ysep, input.outer_xsep, input.outer_ysep, input.line_width);
    // the parts' boxes; an empty part has the size of `every empty part`, or is skipped
    let mut boxes = Vec::new();
    let mut skipped = Vec::new();
    for k in 1..=n {
        let b = input.part(part_name(k));
        let is_empty = b.wd == 0.0 && b.ht == 0.0 && b.dp == 0.0;
        skipped.push(is_empty && ignore_empty);
        boxes.push(if is_empty && !ignore_empty { empty } else { b });
    }
    let mut max_width = boxes.iter().map(|b| b.wd).fold(0.0, f64::max);
    let max_ht = boxes.iter().map(|b| b.ht).fold(0.0, f64::max);
    let max_dp = boxes.iter().map(|b| b.dp).fold(0.0, f64::max);
    let mut max_total = max_ht + max_dp;
    max_width = max_width.max(input.minimum_width - 2.0 * ix);
    max_total = max_total.max(input.minimum_height - 2.0 * iy);
    let align = per_part(&input.keys.text("/pgf/rectangle split part align"), n);
    let mut origins = vec![Point::new(0.0, 0.0); n];
    let mut last = 0usize;
    let (sw, ne);
    let pt = Point::new;
    let mut anchors: Vec<(String, Point)> = Vec::new();
    if horizontal {
        let (mut x, mut y_offset) = (0.0, 0.0);
        for k in 0..n {
            if skipped[k] && k > 0 {
                origins[k] = origins[last];
                continue;
            }
            let b = boxes[k];
            let y = match align[k].as_str() {
                "bottom" => b.dp,
                "top" => max_total - b.ht,
                "base" => (max_total - max_ht + max_dp) / 2.0,
                _ => (max_total - b.ht + b.dp) / 2.0,
            };
            if k == 0 {
                y_offset = y;
            } else {
                x += boxes[last].wd + 2.0 * ix + lw;
                origins[k] = pt(x, y - y_offset);
            }
            last = k;
        }
        ne = pt(origins[last].x + boxes[last].wd + ix + ox, max_total - y_offset + iy + oy);
        sw = pt(origins[0].x - ix - ox, -y_offset - iy - oy);
        let c = sw.lerp(ne, 0.5);
        for k in 0..n {
            if skipped[k] {
                continue;
            }
            let name = anchor_name(k + 1);
            let xa = origins[k].x + boxes[k].wd / 2.0;
            anchors.push((format!("{name} south"), pt(xa, sw.y)));
            anchors.push((format!("{name} north"), pt(xa, ne.y)));
            if k + 1 < n {
                let xs = xa + boxes[k].wd / 2.0 + ix + lw / 2.0;
                anchors.push((format!("{name} split south"), pt(xs, sw.y)));
                anchors.push((format!("{name} split north"), pt(xs, ne.y)));
                anchors.push((format!("{name} split"), pt(xs, c.y)));
            }
        }
    } else {
        let (mut y, mut x_offset) = (0.0, 0.0);
        for k in 0..n {
            if skipped[k] && k > 0 {
                origins[k] = origins[last];
                continue;
            }
            let b = boxes[k];
            let x = match align[k].as_str() {
                "left" => 0.0,
                "right" => max_width - b.wd,
                _ => (max_width - b.wd) / 2.0,
            };
            if k == 0 {
                x_offset = x;
            } else {
                y -= boxes[last].dp + 2.0 * iy + lw + b.ht;
                origins[k] = pt(x - x_offset, y);
            }
            last = k;
        }
        ne = pt(max_width - x_offset + ix + ox, boxes[0].ht + iy + oy + lw / 2.0);
        sw = pt(-x_offset - ix - ox, origins[last].y - boxes[last].dp - iy - oy - lw / 2.0);
        let c = sw.lerp(ne, 0.5);
        for k in 0..n {
            if skipped[k] {
                continue;
            }
            let name = anchor_name(k + 1);
            let ya = origins[k].y + boxes[k].ht / 2.0 - boxes[k].dp / 2.0;
            anchors.push((format!("{name} west"), pt(sw.x, ya)));
            anchors.push((format!("{name} east"), pt(ne.x, ya)));
            if k + 1 < n {
                let ys = origins[k].y - boxes[k].dp - iy - lw / 2.0;
                anchors.push((format!("{name} split west"), pt(sw.x, ys)));
                anchors.push((format!("{name} split east"), pt(ne.x, ys)));
                anchors.push((format!("{name} split"), pt(c.x, ys)));
            }
        }
    }
    let c = sw.lerp(ne, 0.5);
    let mut g = Geometry::new(c, Border::Rectangle(ne.minus(sw).scale(0.5)));
    let wd = input.text().wd;
    for (name, q) in [
        ("center", c),
        ("base", pt(wd / 2.0, 0.0)),
        ("base east", pt(ne.x, 0.0)),
        ("base west", pt(sw.x, 0.0)),
        ("mid", pt(wd / 2.0, 0.5 * ex)),
        ("mid east", pt(ne.x, 0.5 * ex)),
        ("mid west", pt(sw.x, 0.5 * ex)),
        ("north", pt(c.x, ne.y)),
        ("south", pt(c.x, sw.y)),
        ("east", pt(ne.x, c.y)),
        ("west", pt(sw.x, c.y)),
        ("north east", ne),
        ("north west", pt(sw.x, ne.y)),
        ("south west", sw),
        ("south east", pt(ne.x, sw.y)),
    ] {
        g.anchor_at(name, q);
    }
    g.anchors.extend(anchors);
    // the parts' anchors, and the aliases pgf gives them; a missing one falls back to its side
    g.parts.clear();
    for k in 1..=20 {
        let name = anchor_name(k);
        let origin = origins[(k - 1).min(n - 1)];
        let origin = if k <= n { origin } else { origins[0] };
        g.anchor_at(name, origin);
        if k <= n {
            g.parts.push((part_name(k).to_string(), origin));
        }
        for (suffix, side) in [
            ("north", "north"),
            ("south", "south"),
            ("east", "east"),
            ("west", "west"),
            ("split", "center"),
            ("split north", "north"),
            ("split south", "south"),
            ("split east", "east"),
            ("split west", "west"),
        ] {
            let full = format!("{name} {suffix}");
            if !g.has_anchor(&full) {
                let fallback = g.anchor(side).unwrap_or(c);
                g.anchor_at(&full, fallback);
            }
        }
    }
    for (alias, of) in [("text", "one"), ("second", "two"), ("third", "three"), ("fourth", "four")] {
        for suffix in ["", " east", " west", " north", " south", " split", " split east", " split west", " split north", " split south"] {
            if alias != "text" || !suffix.is_empty() {
                let q = g.anchor(&format!("{of}{suffix}")).expect("the anchors of the first twenty parts are all set above");
                g.anchor_at(&format!("{alias}{suffix}"), q);
            }
        }
    }
    // custom fills of the parts
    if input.keys.flag("tikra@sh@splitfill") {
        let colors = per_part(&input.keys.text("/pgf/rectangle split part fill"), n);
        let rect = |a: Point, b: Point| vec![Segment::MoveTo(a), Segment::LineTo(pt(a.x, b.y)), Segment::LineTo(b), Segment::LineTo(pt(b.x, a.y)), Segment::Close];
        if horizontal {
            let mut bl = sw;
            for k in 1..=n {
                let tr = if k == n { ne } else { g.anchor(&format!("{} split north", anchor_name(k))).unwrap_or(ne) };
                let bl2 = bl.plus(pt(if k == 1 { ox } else { 0.0 }, oy));
                let tr2 = tr.plus(pt(if k == n { -ox } else { 0.0 }, -oy));
                if !skipped[k - 1] && colors[k - 1] != "none" {
                    g.path(Layer::BehindBackground, rect(bl2, tr2), Usage::Fill(colors[k - 1].clone()));
                }
                bl = pt(tr2.x, bl2.y - oy);
            }
        } else {
            let mut tr = ne;
            for k in 1..=n {
                let bl = if k == n { sw } else { g.anchor(&format!("{} split west", anchor_name(k))).unwrap_or(sw) };
                let bl2 = bl.plus(pt(ox, if n == 1 || k == n { oy } else { 0.0 }));
                let tr2 = tr.plus(pt(-ox, if n == 1 || k == 1 { -oy } else { 0.0 }));
                if !skipped[k - 1] && colors[k - 1] != "none" {
                    g.path(Layer::BehindBackground, rect(bl2, tr2), Usage::Fill(colors[k - 1].clone()));
                }
                tr = pt(tr2.x + ox, bl2.y);
            }
        }
    }
    let (a, b) = (sw.plus(pt(ox, oy)), ne.minus(pt(ox, oy)));
    let mut v = vec![Segment::MoveTo(a), Segment::LineTo(pt(b.x, a.y)), Segment::LineTo(b), Segment::LineTo(pt(a.x, b.y)), Segment::Close];
    if draw_splits {
        for k in 1..n {
            if skipped[k - 1] {
                continue;
            }
            let name = anchor_name(k);
            let (from, to) = if horizontal {
                (g.anchor(&format!("{name} split north")).unwrap_or(c).minus(pt(0.0, oy)), g.anchor(&format!("{name} split south")).unwrap_or(c).plus(pt(0.0, oy)))
            } else {
                (g.anchor(&format!("{name} split east")).unwrap_or(c).minus(pt(ox, 0.0)), g.anchor(&format!("{name} split west")).unwrap_or(c).plus(pt(ox, 0.0)))
            };
            v.push(Segment::MoveTo(from));
            v.push(Segment::LineTo(to));
        }
    }
    g.background(v);
    g
}
