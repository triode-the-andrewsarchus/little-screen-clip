//! Polar axes (`tikzlibrarypgfplots.polar.code.tex`): x is an angle in degrees, y a radius. The
//! axis is a circle (or a sector from `xmin` to `xmax`), from 0 or `ymin` out to the largest
//! radius, stretched to `width` × `height` (equal by default). Angles tick every 90, 45, 30, 15, 10
//! or 5 degrees (`…normalizetickdist@polar`), the last tick of a full circle is the first one;
//! the grid is rays and arcs, the tick labels sit outside the circle and along the first ray.

use std::f64::consts::PI;

use crate::scene::{Point, Segment};
use crate::{Error, PT};

use super::super::nodes::{NodeSpec, OnPath};
use super::draw::{axis_char, tick_offset, visible, Frame, Setup};
use super::keys;
use super::{toks, Engine};

/// sin and cos of degrees, exact at multiples of 90.
pub fn sin_cos(deg: f64) -> (f64, f64) {
    let r = deg.rem_euclid(360.0);
    if r == 0.0 {
        (0.0, 1.0)
    } else if r == 90.0 {
        (1.0, 0.0)
    } else if r == 180.0 {
        (0.0, -1.0)
    } else if r == 270.0 {
        (-1.0, 0.0)
    } else {
        deg.to_radians().sin_cos()
    }
}

/// Whether [xmin, xmax] is a whole turn (`\pgfplots@xtick@disable@last@tick`).
fn full(xmin: f64, xmax: f64) -> bool {
    let d = (xmax - xmin).rem_euclid(360.0);
    d.abs() < 0.002 || (360.0 - d).abs() < 0.002
}

/// Points of the sector's outline, in units of the data (radius), for its size.
fn outline_points(xmin: f64, xmax: f64, ymin: f64, ymax: f64) -> Vec<(f64, f64)> {
    let mut v = Vec::new();
    let steps = (((xmax - xmin).abs() * 2.0).ceil() as usize).clamp(4, 2000);
    for k in 0..=steps {
        let a = xmin + (xmax - xmin) * k as f64 / steps as f64;
        let (s, c) = sin_cos(a);
        v.push((ymax * c, ymax * s));
        v.push((ymin * c, ymin * s));
    }
    v
}

/// Whether a point of the canvas lies in the sector (`\pgfplotsaxisifcontainspoint@polar`): its
/// radius within [ymin, ymax] and its angle, modulo 360, within [xmin, xmax].
pub fn contains(f: &Frame, p: Point) -> bool {
    let d = p.minus(f.origin);
    let (u, v) = (d.x / f.e[0].x, d.y / f.e[1].y);
    if !(u.is_finite() && v.is_finite()) {
        return true;
    }
    let tol = 0.05 * PT / f.e[1].norm().max(1e-12);
    let r = u.hypot(v);
    if r < f.min[1].abs().min(f.max[1].abs()) - tol || r > f.max[1].abs() + tol {
        return false;
    }
    if r <= tol || full(f.min[0], f.max[0]) {
        return true;
    }
    let a = v.atan2(u).to_degrees().rem_euclid(360.0);
    let (lo, hi) = (f.min[0].rem_euclid(360.0), f.max[0].rem_euclid(360.0));
    let eps = 1e-3;
    if hi > lo {
        a >= lo - eps && a <= hi + eps
    } else {
        // the sector goes round 0
        a <= hi + eps || a >= lo - eps
    }
}

/// The sector's outline on the canvas.
pub fn outline(f: &Frame) -> Vec<Point> {
    outline_points(f.min[0], f.max[0], f.min[1], f.max[1]).into_iter().map(|(x, y)| f.origin.plus(f.e[0].scale(x)).plus(f.e[1].scale(y))).collect()
}

/// An arc of radius `r` from angle `a0` to `a1`, as Bézier pieces of at most 90°.
fn arc(f: &Frame, r: f64, a0: f64, a1: f64, segs: &mut Vec<Segment>, start: bool) {
    let map = |u: f64, v: f64| f.origin.plus(f.e[0].scale(u)).plus(f.e[1].scale(v));
    let n = ((a1 - a0).abs() / 90.0).ceil().max(1.0) as usize;
    let step = (a1 - a0) / n as f64;
    let (s0, c0) = sin_cos(a0);
    if start {
        segs.push(Segment::MoveTo(map(r * c0, r * s0)));
    }
    for i in 0..n {
        let (t0, t1) = (a0 + step * i as f64, a0 + step * (i + 1) as f64);
        let k = 4.0 / 3.0 * ((t1 - t0).to_radians() / 4.0).tan();
        let ((s0, c0), (s1, c1)) = (sin_cos(t0), sin_cos(t1));
        let p1 = map(r * (c0 - k * s0), r * (s0 + k * c0));
        let p2 = map(r * (c1 + k * s1), r * (s1 - k * c1));
        segs.push(Segment::CurveTo(p1, p2, map(r * c1, r * s1)));
    }
}

/// The clip path: the sector (`\pgfplots@clippath@prepare@for@axistype@polaraxis`).
pub fn clip_path(f: &Frame) -> Vec<Segment> {
    let (xmin, xmax, ymin, ymax) = (f.min[0], f.max[0], f.min[1], f.max[1]);
    let mut segs = Vec::new();
    arc(f, ymax, xmin, xmax, &mut segs, true);
    if full(xmin, xmax) {
        if ymin != 0.0 {
            arc(f, ymin, xmin, xmax, &mut segs, true);
        }
    } else if ymin == 0.0 {
        segs.push(Segment::LineTo(f.origin));
    } else {
        let (s, c) = sin_cos(xmax);
        segs.push(Segment::LineTo(f.origin.plus(f.e[0].scale(ymin * c)).plus(f.e[1].scale(ymin * s))));
        arc(f, ymin, xmax, xmin, &mut segs, false);
    }
    segs.push(Segment::Close);
    segs
}

/// The angles' ticks; None for the radius (it ticks as a linear axis).
pub fn ticks(e: &mut Engine, a: usize, f: &Frame) -> Option<(Vec<f64>, f64)> {
    if a != 0 {
        return None;
    }
    // given ticks are read as usual
    let raw = crate::keys::value(&e.tex, "/pgfplots/xtick").unwrap_or_default();
    if !crate::keys::trim(&raw).is_empty() || keys::number(e, "/pgfplots/xtick distance").is_some() {
        return None;
    }
    let (xmin, xmax) = (f.min[0], f.max[0]);
    // the circumference (`\pgfplotspointxaxislength`)
    let mut length = f.e[0].norm() * f.max[1] * 2.0 * PI / PT;
    let frac = (xmax - xmin) / 360.0;
    if frac < 1.0 {
        length *= frac;
    }
    let max_space = keys::number(e, "/pgfplots/max space between ticks").unwrap_or(35.0);
    let try_min = keys::number(e, "/pgfplots/try min ticks").unwrap_or(4.0) as usize;
    let n = ((length / max_space).floor() as usize + 1).max(try_min);
    // H = (MAX + 0.001pt - MIN) / (n-1), as TeX divides
    let sp = |v: f64| (v * 65536.0).round();
    let h = ((sp(xmax) + sp(0.001) - sp(xmin)) / (n.max(2) - 1) as f64).trunc() / 65536.0;
    let h = match h {
        h if h > 67.0 => 90.0,
        h if h > 30.0 => 45.0,
        h if h > 18.0 => 30.0,
        h if h > 10.0 => 15.0,
        h if h > 6.0 => 10.0,
        _ => 5.0,
    };
    let settings = super::ticks::TickSettings { length_pt: length, max_space, try_min, distance: Some(h), scale: None };
    let (mut list, d) = super::ticks::linear(xmin, xmax, settings);
    // a whole turn: the last tick is the first
    if full(xmin, xmax) {
        list.retain(|t| (t - xmax).abs() > 1e-6 * h);
    }
    Some((list, d))
}

/// The polar axis: grid, ticks, axis lines and tick labels.
pub(super) fn draw_axis(e: &mut Engine, s: &Setup) -> Result<(), Error> {
    let f = s.frame.clone();
    let (xmin, xmax, ymin, ymax) = (f.min[0], f.max[0], f.min[1], f.max[1]);
    let xt: Vec<f64> = if s.hide[0] { Vec::new() } else { visible(&f, 0, &s.ticks[0]).into_iter().map(|(_, t)| t).collect() };
    let yt: Vec<f64> = if s.hide[1] { Vec::new() } else { visible(&f, 1, &s.ticks[1]).into_iter().map(|(_, t)| t).collect() };
    // grid: rays at the angles, arcs at the radii
    if keys::flag(e, "/pgfplots/xmajorgrids") {
        let segs: Vec<Segment> = xt.iter().flat_map(|t| [Segment::MoveTo(f.at([*t, ymin, 0.0])), Segment::LineTo(f.at([*t, ymax, 0.0]))]).collect();
        e.draw_with("/pgfplots/every axis grid,/pgfplots/every major grid,/pgfplots/every axis x grid,/pgfplots/every major x grid", segs)?;
    }
    if keys::flag(e, "/pgfplots/ymajorgrids") {
        let mut segs = Vec::new();
        for t in &yt {
            arc(&f, *t, xmin, xmax, &mut segs, true);
        }
        e.draw_with("/pgfplots/every axis grid,/pgfplots/every major grid,/pgfplots/every axis y grid,/pgfplots/every major y grid", segs)?;
    }
    // axis lines: the arcs at ymin and ymax, the rays at xmin and xmax
    let mut segs = Vec::new();
    arc(&f, ymax, xmin, xmax, &mut segs, true);
    if ymin > 0.0 {
        arc(&f, ymin, xmin, xmax, &mut segs, true);
    }
    if !s.hide[0] {
        e.draw_with("/pgfplots/every outer x axis line", segs)?;
    }
    if !s.hide[1] {
        let mut segs = Vec::new();
        for a in [xmin, xmax] {
            segs.push(Segment::MoveTo(f.at([a, ymin, 0.0])));
            segs.push(Segment::LineTo(f.at([a, ymax, 0.0])));
        }
        e.draw_with("/pgfplots/every outer y axis line", segs)?;
    }
    // tick labels: angles outside the circle, radii along the first ray
    for (a, list) in [(0usize, &xt), (1usize, &yt)] {
        if s.hide[a] || !keys::flag(e, &format!("/pgfplots/{}majorticks", axis_char(a))) {
            continue;
        }
        let c = axis_char(a);
        let styles = format!("/pgfplots/every tick label,/pgfplots/every {c} tick label");
        let off = tick_offset(e, a);
        // the style is always defined (pgfplots' defaults)
        let sloped = matches!(e.tex.meaning_of(&crate::keys::macro_name(&format!("/pgfplots/every {c} tick label/.@cmd"))),
            crate::tex::Meaning::Macro(m) if crate::tex::tokens_to_string(&crate::tex::substitute(&m.body, &[])).contains("sloped"));
        for (k, t) in list.iter().enumerate() {
            let (p, out) = if a == 0 {
                let p = f.at([*t, ymax, 0.0]);
                (p, super::draw::unit(p.minus(f.origin)))
            } else {
                let (sn, cs) = sin_cos(xmin);
                let n = super::draw::unit(f.e[0].scale(-sn).plus(f.e[1].scale(cs)));
                (f.at([xmin, *t, 0.0]), n)
            };
            let at = p.plus(out.scale(off));
            let shown = if a == 0 && *t > 360.0 { t.rem_euclid(360.0) } else { *t };
            e.begin_group();
            let r = (|| {
                e.keys_tikz(&styles)?;
                super::draw::tick_text_for(e, a, shown, k)
            })();
            e.end_group();
            let text = r?;
            // the node's border faces the axis: an angle anchor against the outward direction
            let mut angle = (-out.y).atan2(-out.x).to_degrees();
            // `sloped like x axis` turns an angle's label along the circle, never upside down:
            // the anchor turns with it
            if a == 0 && sloped {
                let (sn, cs) = sin_cos(*t);
                let mut d = f.e[0].scale(-sn).plus(f.e[1].scale(cs));
                if d.x < 0.0 {
                    d = d.scale(-1.0);
                }
                angle -= d.y.atan2(d.x).to_degrees();
                f.slope.set(Some(d));
            }
            let options = toks(&format!("anchor={angle},{styles}"));
            let items = e.node(NodeSpec { options, name: None, at: Some(at), text, place: OnPath::Point(at), point: at, coordinate: false, default_pos: None });
            f.slope.set(None);
            e.canvas.items.extend(items?);
        }
    }
    Ok(())
}
