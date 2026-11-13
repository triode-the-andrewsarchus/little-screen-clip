//! Smith charts (`tikzlibrarypgfplots.smithchart.code.tex`): a point `x + j·y` (an impedance) is
//! drawn at its reflection coefficient `(z − 1)/(z + 1)` in the unit circle (stretched to `width`
//! and `height`, or taken as it is with `is smithchart cs`). x grid lines are circles of constant
//! x, y grid lines arcs of constant y; both may stop early at a line of the other kind (`xgrid
//! each nth passes y`, `ygrid stop at x` and their friends) so that the chart stays readable near
//! (1, 0). The x axis is the diameter with its tick labels above it, the y axis the outer circle
//! with its labels around it. The ticks depend on the size unless given.

use std::collections::HashMap;

use crate::scene::{Point, Segment};
use crate::tex::Token;
use crate::{Error, PT};

use super::super::nodes::{NodeSpec, OnPath};
use super::draw::{axis_char, minor_ticks, number_list, tick_labels_at, tick_offset, unit, visible, Frame, Setup};
use super::keys;
use super::{toks, Engine};

/// `(a + jb) / (c + jd)`.
fn divide(a: f64, b: f64, c: f64, d: f64) -> (f64, f64) {
    let s = 1.0 / (c * c + d * d);
    ((a * c + b * d) * s, (b * c - a * d) * s)
}

/// The reflection coefficient of `x + j·y`.
pub fn gamma(x: f64, y: f64) -> (f64, f64) {
    divide(x - 1.0, y, x + 1.0, y)
}

/// The impedance whose reflection coefficient is `u + j·v` (`is smithchart cs` input).
pub fn impedance(u: f64, v: f64) -> (f64, f64) {
    divide(1.0 + u, v, 1.0 - u, -v)
}

/// The canvas point of a point of the unit circle's plane.
pub fn canvas(f: &Frame, u: f64, v: f64) -> Point {
    f.origin.plus(f.e[0].scale(u)).plus(f.e[1].scale(v))
}

/// The canvas point of logical coordinates (an impedance).
pub fn at(f: &Frame, l: [f64; 3]) -> Point {
    let (u, v) = gamma(l[0], l[1]);
    canvas(f, u, v)
}

/// The chart's bounding box.
pub fn bbox(f: &Frame) -> (Point, Point) {
    let (rx, ry) = (f.e[0].norm(), f.e[1].norm());
    (Point::new(f.origin.x - rx, f.origin.y - ry), Point::new(f.origin.x + rx, f.origin.y + ry))
}

/// The outward normal of the y axis (the outer circle) at the tick of value `t`.
fn normal_at(f: &Frame, t: f64) -> Point {
    let (u, v) = gamma(0.0, t);
    unit(canvas(f, u, v).minus(f.origin))
}

/// The outer normals of the axes: up for x, at y = 0 for y (`…outernormalvectorofaxis@smithchartaxis`).
pub fn normal(f: &Frame, a: usize) -> Point {
    if a == 0 {
        Point::new(0.0, 1.0)
    } else {
        normal_at(f, 0.0)
    }
}

/// An arc of the plane of the unit circle (center, radius, angles in degrees), as Bézier pieces
/// of at most 90°, mapped to the canvas.
fn arc(f: &Frame, c: (f64, f64), r: f64, a0: f64, a1: f64, segs: &mut Vec<Segment>) {
    let n = ((a1 - a0).abs() / 90.0).ceil().max(1.0) as usize;
    let step = (a1 - a0) / n as f64;
    let at = |t: f64| {
        let (s, co) = t.to_radians().sin_cos();
        (c.0 + r * co, c.1 + r * s)
    };
    let p = at(a0);
    segs.push(Segment::MoveTo(canvas(f, p.0, p.1)));
    for i in 0..n {
        let (t0, t1) = (a0 + step * i as f64, a0 + step * (i + 1) as f64);
        let k = 4.0 / 3.0 * ((t1 - t0).to_radians() / 4.0).tan();
        let ((s0, c0), (s1, c1)) = (t0.to_radians().sin_cos(), t1.to_radians().sin_cos());
        let p1 = (c.0 + r * (c0 - k * s0), c.1 + r * (s0 + k * c0));
        let p2 = (c.0 + r * (c1 + k * s1), c.1 + r * (s1 - k * c1));
        let q = at(t1);
        segs.push(Segment::CurveTo(canvas(f, p1.0, p1.1), canvas(f, p2.0, p2.1), canvas(f, q.0, q.1)));
    }
}

/// The angle of a point of a circle around `c` (`\pgfplots@compute@angle@of@point@in@circle`).
fn angle(p: (f64, f64), c: (f64, f64), r: f64) -> f64 {
    let (d, dim) = (p.0 - c.0, p.1 - c.1);
    let a = (d / r).clamp(-1.0, 1.0).acos().to_degrees();
    if dim < 0.0 {
        360.0 - a
    } else {
        a
    }
}

/// The circle of constant x, to where y reaches `stop` (all of it without).
fn x_circle(f: &Frame, x: f64, stop: Option<f64>, segs: &mut Vec<Segment>) {
    let r = 1.0 / (1.0 + x);
    let c = (x * r, 0.0);
    let (a0, a1) = match stop {
        None => (0.0, 360.0),
        Some(s) => {
            let a = angle(gamma(x, s), c, r);
            (a.min(360.0 - a), a.max(360.0 - a))
        }
    };
    arc(f, c, r, a0, a1, segs);
}

/// The arc of constant y, from the outer circle to (1, 0) or to the circle x = `stop`.
fn y_arc(f: &Frame, y: f64, stop: Option<f64>, segs: &mut Vec<Segment>) {
    if y.abs() < 0.002 {
        segs.push(Segment::MoveTo(canvas(f, -1.0, 0.0)));
        segs.push(Segment::LineTo(canvas(f, 1.0, 0.0)));
        return;
    }
    let r = 1.0 / y.abs();
    let c = (1.0, 1.0 / y);
    let start = gamma(0.0, y);
    let a0 = angle(start, c, r);
    let a1 = match stop {
        None => {
            if start.1 > 0.0 {
                270.0
            } else {
                90.0
            }
        }
        Some(s) => angle(gamma(s, y), c, r),
    };
    arc(f, c, r, a0, a1, segs);
}

/// `xgrid each nth passes y` entries: a line of the other kind, which lines pass it (each nth by
/// their order of magnitude) and for which values (`if <`).
struct Rule {
    at: f64,
    nth: i64,
    below: Option<f64>,
}

fn rules(e: &mut Engine, text: &str, other: &[f64]) -> Vec<Rule> {
    let mut out = Vec::new();
    for entry in text.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let compact = entry.replace("if <", "if<");
        let (head, below) = match compact.split_once("if<") {
            Some((h, b)) => (h.trim().to_string(), e.number(b.trim()).ok()),
            None => (compact.trim().to_string(), None),
        };
        let (at, nth) = match head.split_once(':') {
            Some((a, n)) => (a.trim().to_string(), n.trim().parse().unwrap_or(2)),
            None => (head, 2),
        };
        // `[index]3`: the other axis' fourth grid line
        let at = match at.strip_prefix("[index]") {
            Some(i) => match i.trim().parse::<usize>().ok().and_then(|i| other.get(i)) {
                Some(v) => *v,
                None => {
                    e.warn(format!("smithchart: there is no grid line '{at}': entry skipped"));
                    continue;
                }
            },
            None => match e.number(&at) {
                Ok(v) => v,
                Err(_) => continue,
            },
        };
        out.push(Rule { at, nth: nth.max(1), below });
    }
    out
}

/// A grid line's value as the stop maps know it: two decimals.
fn key(v: f64) -> i64 {
    (v * 100.0).round() as i64
}

/// The grid lines of one axis and what their stops depend on.
struct Lines {
    /// (tick number, value): the major lines, then the minor ones
    lines: Vec<(usize, f64)>,
    rules: Vec<Rule>,
    /// a line's number to its place by magnitude
    order: HashMap<usize, i64>,
    /// the given and the computed stops (y's are symmetric)
    stops: HashMap<i64, f64>,
    /// where each line stops, as computed
    last: HashMap<i64, f64>,
}

impl Lines {
    fn new(e: &mut Engine, a: usize, major: &[(usize, f64)], minor: &[(usize, f64)], other: &[f64]) -> Lines {
        let c = axis_char(a);
        let o = axis_char(1 - a);
        let mut lines: Vec<(usize, f64)> = Vec::new();
        if keys::flag(e, &format!("/pgfplots/{c}majorgrids")) {
            lines.extend(major.iter().copied());
        }
        if keys::flag(e, &format!("/pgfplots/{c}minorgrids")) {
            lines.extend(minor.iter().copied());
        }
        let text = keys::value(e, &format!("/pgfplots/{c}grid each nth passes {o}"));
        let rules = rules(e, &text, other);
        // the places by magnitude: equal magnitudes share one, zero always has the first
        let mut order = HashMap::new();
        if !rules.is_empty() {
            let mut sorted: Vec<(Option<usize>, f64)> = vec![(None, 0.0)];
            sorted.extend(lines.iter().map(|(i, v)| (Some(*i), v.abs())));
            sorted.sort_by(|a, b| a.1.total_cmp(&b.1));
            let mut k = keys::number(e, &format!("/pgfplots/{c}grid each nth passes {o} start")).unwrap_or(0.0) as i64;
            let mut last: Option<f64> = None;
            for (i, v) in sorted {
                if last.is_some_and(|l| key(l) != key(v) || l != v) {
                    k += 1;
                }
                if let Some(i) = i {
                    order.insert(i, k);
                }
                last = Some(v);
            }
        }
        // `ygrid stop at x={0.5:0.5 -0.2:0.2}`
        let mut stops = HashMap::new();
        let text = keys::value(e, &format!("/pgfplots/{c}grid stop at {o}"));
        for entry in text.split([',', ' ']).map(str::trim).filter(|s| !s.is_empty()) {
            if let Some((v, s)) = entry.split_once(':') {
                if let (Ok(v), Ok(s)) = (e.number(v.trim()), e.number(s.trim())) {
                    stops.insert(key(v), s);
                    if a == 1 {
                        stops.entry(key(-v)).or_insert(s);
                    }
                }
            }
        }
        Lines { lines, rules, order, stops, last: HashMap::new() }
    }

    /// Where line `(num, t)` stops by the rules; `other` (the other axis' computed stops) rejects a
    /// stop at a line that ends before reaching this one.
    fn compute(&mut self, a: usize, finalize: bool, other: Option<&HashMap<i64, f64>>) {
        for (num, t) in self.lines.clone() {
            let mut stop = self.stops.get(&key(t)).copied();
            if stop.is_none() && !self.rules.is_empty() {
                let mut k = self.order.get(&num).copied().unwrap_or(0);
                for r in &self.rules {
                    if stop.is_some() {
                        break;
                    }
                    if r.below.is_some_and(|b| t.abs() >= b) {
                        continue;
                    }
                    if k % r.nth == 0 {
                        k /= r.nth;
                        continue;
                    }
                    stop = Some(r.at);
                    if let (true, Some(o)) = (finalize, other) {
                        if o.get(&key(r.at)).is_some_and(|q| t.abs() >= *q) {
                            stop = None;
                        }
                    }
                }
            }
            if let Some(s) = stop {
                self.last.insert(key(t), s);
                if finalize {
                    self.stops.insert(key(t), s);
                    if a == 1 {
                        self.stops.insert(key(-t), s);
                    }
                }
            }
        }
    }
}

/// The tick numbers pgfplots gives the minor ticks (`\pgfplots@prepare@tick@coordlists@for`): a
/// `minor xtick` list counts on after the whole major list, a tick of `minor x tick num` is its
/// major tick's number plus its own (so it shares a number with a later major tick, as in pgfplots).
fn minor_numbers(e: &mut Engine, s: &Setup, a: usize, minor: &[f64]) -> Vec<(usize, f64)> {
    let f = &s.frame;
    let all = &s.ticks[a];
    let given = keys::value_tokens(e, &format!("/pgfplots/minor {}tick", axis_char(a)));
    if let Some(v) = given {
        let list: Vec<f64> = number_list(e, v).unwrap_or_default().into_iter().map(|x| f.logical(a, x)).collect();
        return minor.iter().map(|t| (all.len() + list.iter().position(|x| x == t).unwrap_or(0), *t)).collect();
    }
    minor
        .iter()
        .map(|t| {
            let j = all.iter().rposition(|m| m < t).unwrap_or(0);
            let next = all.get(j + 1).copied().unwrap_or(*t);
            let n = keys::number(e, &format!("/pgfplots/minor {} tick num", axis_char(a))).unwrap_or(1.0).max(1.0);
            let i = if next > all[j] { ((t - all[j]) / ((next - all[j]) / (n + 1.0))).round() as usize } else { 1 };
            (j + i, *t)
        })
        .collect()
}

/// The clip path: the chart's circle.
pub fn clip_path(f: &Frame) -> Vec<Segment> {
    let mut segs = Vec::new();
    arc(f, (0.0, 0.0), 1.0, 0.0, 360.0, &mut segs);
    segs.push(Segment::Close);
    segs
}

/// The ticks pgfplots picks for a chart of this width (`smithchart ticks by size`).
pub fn size_style(width: f64) -> &'static str {
    if width < 14.0 {
        "few smithchart ticks"
    } else if width < 20.0 {
        "many smithchart ticks"
    } else {
        "dense smithchart ticks"
    }
}

/// The chart: grid lines, ticks, the axis lines and the tick labels.
pub(super) fn draw_axis(e: &mut Engine, s: &Setup) -> Result<(), Error> {
    let f = s.frame.clone();
    let major_n: [Vec<(usize, f64)>; 2] = [0, 1].map(|a| if s.hide[a] { Vec::new() } else { visible(&f, a, &s.ticks[a]) });
    let major: [Vec<f64>; 2] = [0, 1].map(|a| major_n[a].iter().map(|(_, t)| *t).collect());
    let minor: [Vec<f64>; 2] = [minor_ticks(e, s, 0, &s.ticks[0]), minor_ticks(e, s, 1, &s.ticks[1])];
    let minor_n = [minor_numbers(e, s, 0, &minor[0]), minor_numbers(e, s, 1, &minor[1])];
    // the stops: x lines first without, then y lines, then x lines with the constraints
    let all = |a: usize| -> Vec<f64> { major[a].iter().chain(minor[a].iter()).copied().collect() };
    let mut lx = Lines::new(e, 0, &major_n[0], &minor_n[0], &all(1));
    let mut ly = Lines::new(e, 1, &major_n[1], &minor_n[1], &all(0));
    lx.compute(0, false, None);
    ly.compute(1, true, Some(&lx.last));
    lx.compute(0, true, Some(&ly.last));
    for (a, lines) in [(0usize, &lx), (1usize, &ly)] {
        let c = axis_char(a);
        for (kind, list) in [("minor", &minor[a]), ("major", &major[a])] {
            if !keys::flag(e, &format!("/pgfplots/{c}{kind}grids")) || list.is_empty() {
                continue;
            }
            let mut segs = Vec::new();
            for t in list {
                let stop = lines.stops.get(&key(*t)).copied();
                if a == 0 {
                    x_circle(&f, *t, stop, &mut segs);
                } else {
                    y_arc(&f, *t, stop, &mut segs);
                }
            }
            e.draw_with(&format!("/pgfplots/every axis grid,/pgfplots/every {kind} grid,/pgfplots/every axis {c} grid,/pgfplots/every {kind} {c} grid"), segs)?;
        }
    }
    // ticks: across the diameter and across the circle
    for a in 0..2 {
        let c = axis_char(a);
        let align = e.pv(&format!("{c}tick align"));
        for (kind, list) in [("minor", &minor[a]), ("major", &major[a])] {
            if !keys::flag(e, &format!("/pgfplots/{c}{kind}ticks")) || list.is_empty() {
                continue;
            }
            let len = keys::length(e, &format!("/pgfplots/{kind} tick length")).unwrap_or(if kind == "major" { 0.15 } else { 0.1 });
            let (from, to) = match align.as_str() {
                "outside" => (0.0, len),
                "center" => (-len / 2.0, len / 2.0),
                _ => (-len, 0.0),
            };
            let mut segs = Vec::new();
            for t in list {
                let (p, n) = if a == 0 {
                    let (u, v) = gamma(*t, 0.0);
                    (canvas(&f, u, v), Point::new(0.0, 1.0))
                } else {
                    let (u, v) = gamma(0.0, *t);
                    (canvas(&f, u, v), normal_at(&f, *t))
                };
                segs.push(Segment::MoveTo(p.plus(n.scale(from))));
                segs.push(Segment::LineTo(p.plus(n.scale(to))));
            }
            e.draw_with(&format!("/pgfplots/every tick,/pgfplots/every {kind} tick,/pgfplots/every {c} tick,/pgfplots/every {kind} {c} tick"), segs)?;
        }
    }
    // the axis lines: the diameter and the outer circle
    if !s.hide[0] {
        let segs = vec![Segment::MoveTo(canvas(&f, -1.0, 0.0)), Segment::LineTo(canvas(&f, 1.0, 0.0))];
        e.draw_with("/pgfplots/every inner x axis line", segs)?;
    }
    if !s.hide[1] {
        let mut segs = Vec::new();
        x_circle(&f, 0.0, None, &mut segs);
        e.draw_with("/pgfplots/every outer y axis line", segs)?;
    }
    // tick labels: above the diameter, around the circle
    if !s.hide[0] && keys::flag(e, "/pgfplots/xmajorticks") {
        let off = tick_offset(e, 0) + keys::length(e, "/pgfplots/xticklabel shift").unwrap_or(0.0);
        let g = f.clone();
        let base = move |t: f64| {
            let (u, v) = gamma(t, 0.0);
            canvas(&g, u, v)
        };
        tick_labels_at(e, s, 0, &s.ticks[0], false, &base, Point::new(0.0, off), off)?;
    }
    if !s.hide[1] && keys::flag(e, "/pgfplots/ymajorticks") {
        let off = tick_offset(e, 1) + keys::length(e, "/pgfplots/yticklabel shift").unwrap_or(0.0);
        let styles = "/pgfplots/every tick label,/pgfplots/every y tick label";
        for (k, t) in s.ticks[1].iter().enumerate() {
            if !major[1].contains(t) {
                continue;
            }
            let (u, v) = gamma(0.0, *t);
            let n = normal_at(&f, *t);
            let at = canvas(&f, u, v).plus(n.scale(off));
            // `sloped like y axis` follows the circle here
            f.slope.set(Some(Point::new(-n.y, n.x)));
            f.upside_down.replace(None);
            e.begin_group();
            let r = (|| {
                e.keys_tikz(styles)?;
                super::draw::tick_text_for(e, 1, *t, k)
            })();
            e.end_group();
            let text = r?;
            // the node faces the axis; an upside down label may change its anchor last
            let mut options = toks(&format!("anchor=near yticklabel,{styles}"));
            if let Some(late) = f.upside_down.take() {
                options.extend(toks(","));
                options.extend(late);
            }
            let items = e.node(NodeSpec { options, name: None, at: Some(at), text, place: OnPath::Point(at), point: at, coordinate: false, default_pos: None });
            f.slope.set(None);
            e.canvas.items.extend(items?);
        }
    }
    Ok(())
}

/// `show origin`: a small ring at the chart's center, over everything.
pub(super) fn show_origin(e: &mut Engine, f: &Frame) -> Result<(), Error> {
    let c = f.origin;
    for (r, style) in [(2.5 * PT, "draw=black,fill=white"), (0.5 * PT, "fill=black")] {
        let mut segs = Vec::new();
        let k = 0.552_284_75 * r;
        let p = |x: f64, y: f64| Point::new(c.x + x, c.y + y);
        segs.push(Segment::MoveTo(p(r, 0.0)));
        segs.push(Segment::CurveTo(p(r, k), p(k, r), p(0.0, r)));
        segs.push(Segment::CurveTo(p(-k, r), p(-r, k), p(-r, 0.0)));
        segs.push(Segment::CurveTo(p(-r, -k), p(-k, -r), p(0.0, -r)));
        segs.push(Segment::CurveTo(p(k, -r), p(r, -k), p(r, 0.0)));
        segs.push(Segment::Close);
        e.begin_group();
        let r = (|| {
            e.st.mode = Default::default();
            e.process_keys(toks(style), Some("/tikz"))?;
            e.use_path(segs, "show origin")
        })();
        e.end_group();
        r?;
    }
    Ok(())
}

/// The options of `sloped like y axis={…}`: the code for an upside down label, whether it may
/// stay upside down and whether rotations given before are dropped.
pub fn sloped_options(v: &[Token]) -> (Option<Vec<Token>>, bool, bool) {
    let mut code = None;
    let mut allow = false;
    let mut reset = true;
    for item in crate::keys::split_list(v) {
        let value = item.value.clone().unwrap_or_default();
        let text = crate::tex::tokens_to_string(&value);
        match item.key.trim() {
            "execute for upside down" => code = Some(value),
            "allow upside down" => allow = text.trim() != "false",
            "reset nontranslations" => reset = text.trim() != "false",
            _ => {}
        }
    }
    (code, allow, reset)
}
