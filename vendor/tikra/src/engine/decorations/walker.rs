//! How pgf walks a path to decorate it (`pgfmoduledecorations.code.tex`): the path is parsed into
//! input segments (lines, curves, closepaths; only the last of several movetos is kept), then an
//! automaton moves along it by distances, keeping the distances pgf keeps (completed and remaining
//! on the whole path, on the current section, on the current input segment) and, on curves, the
//! curve time found by pgf's own bisection. Everything is in pt, in canvas coordinates.

use crate::scene::{Point, Segment, Transform};

/// One scaled point in pt.
pub const SP: f64 = 1.0 / 65536.0;
/// Comparisons of distances allow for float noise where TeX's dimensions are exact.
pub const EPS: f64 = 1e-7;

/// A length as TeX keeps it: a whole number of sp.
pub fn to_sp(pt: f64) -> f64 {
    (pt * 65536.0).round() / 65536.0
}

/// A computed distance as a TeX dimension takes it (`.5\dimen` truncates toward zero): the
/// automaton moves by these, so stretches that seem to end on a corner stop a few sp before it.
pub fn trunc_sp(pt: f64) -> f64 {
    (pt * 65536.0 + if pt >= 0.0 { 1e-6 } else { -1e-6 }).trunc() / 65536.0
}

/// An input segment object (`\pgf@decorate@inputsegmentobject@…`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Obj {
    MoveTo(Point),
    Line { len: f64, a: Point, b: Point },
    Curve { len: f64, a: Point, s1: Point, s2: Point, b: Point },
    Close { len: f64, a: Point, b: Point },
    End,
}

/// What the current input segment is (`\pgfdecorationcurrentinputsegment`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    MoveTo,
    LineTo,
    CurveTo,
    ClosePath,
    Last,
}

/// The current input segment's points, as the object macros define them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SegInfo {
    pub kind: Kind,
    pub first: Point,
    pub sa: Point,
    pub sb: Point,
    pub last: Point,
    pub len: f64,
    /// moving and transforming as on a curve (else as on a line); a moveto or the end leaves it
    pub curve: bool,
}

impl Default for SegInfo {
    fn default() -> Self {
        SegInfo { kind: Kind::Last, first: Point::default(), sa: Point::default(), sb: Point::default(), last: Point::default(), len: 0.0, curve: false }
    }
}

fn thirds(a: Point, b: Point) -> (Point, Point) {
    let d = b.minus(a);
    (a.plus(d.scale(0.333333)), a.plus(d.scale(0.666666)))
}

impl SegInfo {
    /// The segment after executing `obj` (fields it does not set stay as they were).
    fn after(&self, obj: &Obj) -> SegInfo {
        let mut s = *self;
        match *obj {
            Obj::MoveTo(p) => {
                s = SegInfo { kind: Kind::MoveTo, first: p, sa: p, sb: p, last: p, len: 0.0, curve: self.curve };
            }
            Obj::Line { len, a, b } | Obj::Close { len, a, b } => {
                let (sa, sb) = thirds(a, b);
                let kind = if matches!(obj, Obj::Close { .. }) { Kind::ClosePath } else { Kind::LineTo };
                s = SegInfo { kind, first: a, sa, sb, last: b, len, curve: false };
            }
            Obj::Curve { len, a, s1, s2, b } => s = SegInfo { kind: Kind::CurveTo, first: a, sa: s1, sb: s2, last: b, len, curve: true },
            Obj::End => s.kind = Kind::Last,
        }
        s
    }
}

/// `\pgfmathanglebetweenpoints`: degrees in [0, 360).
pub fn angle_between(p: Point, q: Point) -> f64 {
    let d = q.minus(p);
    let a = d.y.atan2(d.x).to_degrees();
    if a < 0.0 {
        a + 360.0
    } else {
        a
    }
}

/// `\pgfpointnormalised`: the direction of `v` (a zero vector points right, as atan2(0,0) = 0).
pub fn normalised(v: Point) -> Point {
    let a = v.y.atan2(v.x);
    Point::new(a.cos(), a.sin())
}

/// The point at time `t` of a curve, and the tangent pgf derives from de Casteljau's second level.
pub fn curve_at(t: f64, p0: Point, p1: Point, p2: Point, p3: Point) -> (Point, Point) {
    let s = t;
    let u = 1.0 - t;
    let l = |a: Point, b: Point| a.scale(u).plus(b.scale(s));
    let (x, xa, xb) = (l(p0, p1), l(p1, p2), l(p2, p3));
    let (x, xa) = (l(x, xa), l(xa, xb));
    (l(x, xa), xa.minus(x))
}

fn split(p: [Point; 4]) -> ([Point; 4], [Point; 4]) {
    let m = |a: Point, b: Point| a.lerp(b, 0.5);
    let (a, b, c) = (m(p[0], p[1]), m(p[1], p[2]), m(p[2], p[3]));
    let (d, e) = (m(a, b), m(b, c));
    let f = m(d, e);
    ([p[0], a, d, f], [f, e, c, p[3]])
}

fn chord_length(p: [Point; 4], depth: usize) -> f64 {
    let d = p[3].minus(p[0]);
    // subdivide until the chord is under 1pt in both directions
    if (d.x.abs() < 1.0 && d.y.abs() < 1.0) || depth > 40 {
        return d.norm();
    }
    let (l, r) = split(p);
    chord_length(l, depth + 1) + chord_length(r, depth + 1)
}

/// `\pgf@decorate@curvelength`: always one subdivision, then chords under 1pt.
pub fn curve_length(p0: Point, p1: Point, p2: Point, p3: Point) -> f64 {
    let (l, r) = split([p0, p1, p2, p3]);
    chord_length(l, 0) + chord_length(r, 0)
}

/// A parsed path (`\pgf@decorate@parsesoftpath`).
#[derive(Debug, Clone, Default)]
pub struct Parsed {
    pub objs: Vec<Obj>,
    pub total: f64,
    pub first: Point,
    pub last_nonmove: Point,
}

/// Parses segments (pt); `existing_last` is where a path not starting with a moveto starts.
pub fn parse(segs: &[Segment], existing_last: Point) -> Parsed {
    let mut p = Parsed::default();
    let mut last = existing_last;
    let mut start = existing_last;
    let mut queue: Option<Obj> = None;
    let mut first: Option<Point> = None;
    let mut add = |p: &mut Parsed, queue: &mut Option<Obj>, obj: Obj| {
        for o in queue.take().into_iter().chain(std::iter::once(obj)) {
            // (the ends are pushed after the segments, never through here)
            if let (None, Obj::MoveTo(q) | Obj::Line { a: q, .. } | Obj::Curve { a: q, .. } | Obj::Close { a: q, .. }) = (first, o) {
                first = Some(q);
            }
            p.objs.push(o);
        }
    };
    for s in segs {
        match *s {
            Segment::MoveTo(q) => {
                queue = Some(Obj::MoveTo(q));
                last = q;
                start = q;
            }
            Segment::LineTo(q) => {
                let len = to_sp(q.minus(last).norm());
                p.total += len;
                add(&mut p, &mut queue, Obj::Line { len, a: last, b: q });
                last = q;
                p.last_nonmove = q;
            }
            Segment::CurveTo(c1, c2, q) => {
                let len = to_sp(curve_length(last, c1, c2, q));
                p.total += len;
                add(&mut p, &mut queue, Obj::Curve { len, a: last, s1: c1, s2: c2, b: q });
                last = q;
                p.last_nonmove = q;
            }
            Segment::Close => {
                let len = to_sp(start.minus(last).norm());
                p.total += len;
                add(&mut p, &mut queue, Obj::Close { len, a: last, b: start });
                last = start;
                p.last_nonmove = start;
            }
        }
    }
    // a final moveto is dropped
    p.objs.push(Obj::End);
    p.objs.push(Obj::End);
    p.first = first.unwrap_or_default();
    p
}

/// `\pgf@decorate@inputsegmentobjects@reverse` (`reverse path`).
pub fn reverse(objs: &[Obj]) -> Vec<Obj> {
    // (the ends come first backwards: nothing follows them, they add nothing)
    let body: Vec<Obj> = objs.iter().rev().copied().collect();
    let mut out = Vec::new();
    let mut moveto_waiting = true;
    let mut close_waiting = false;
    let mut close_len = 0.0;
    let mut last = Point::default();
    for (i, o) in body.iter().enumerate() {
        let next_is_moveto = matches!(body.get(i + 1), Some(Obj::MoveTo(_)));
        let mut line = |len: f64, a: Point, b: Point, out: &mut Vec<Obj>, moveto_waiting: &mut bool, close_waiting: &mut bool| {
            if *moveto_waiting {
                out.push(Obj::MoveTo(b));
                *moveto_waiting = false;
            }
            if next_is_moveto && *close_waiting {
                out.push(Obj::Close { len, a: b, b: a });
                *close_waiting = false;
            } else {
                out.push(Obj::Line { len, a: b, b: a });
            }
            last = a;
        };
        match *o {
            Obj::MoveTo(p) => {
                if close_waiting {
                    out.push(Obj::Close { len: close_len, a: last, b: p });
                    close_waiting = false;
                }
                moveto_waiting = true;
            }
            Obj::Close { len, a, b } => {
                close_waiting = true;
                close_len = len;
                line(len, a, b, &mut out, &mut moveto_waiting, &mut close_waiting);
            }
            Obj::Line { len, a, b } => line(len, a, b, &mut out, &mut moveto_waiting, &mut close_waiting),
            Obj::Curve { len, a, s1, s2, b } => {
                if moveto_waiting {
                    out.push(Obj::MoveTo(b));
                    moveto_waiting = false;
                }
                out.push(Obj::Curve { len, a: b, s1: s2, s2: s1, b: a });
                last = a;
            }
            Obj::End => {}
        }
    }
    out.push(Obj::End);
    out.push(Obj::End);
    out
}

/// The path the automaton leaves (pt, canvas).
#[derive(Debug, Clone, Default)]
pub struct Out {
    pub segs: Vec<Segment>,
    pub last: Point,
    pub start: Point,
}

impl Out {
    pub fn move_to(&mut self, p: Point) {
        self.segs.push(Segment::MoveTo(p));
        self.last = p;
        self.start = p;
    }
    pub fn line_to(&mut self, p: Point) {
        self.segs.push(Segment::LineTo(p));
        self.last = p;
    }
    pub fn curve_to(&mut self, a: Point, b: Point, p: Point) {
        self.segs.push(Segment::CurveTo(a, b, p));
        self.last = p;
    }
    pub fn close(&mut self) {
        self.segs.push(Segment::Close);
        self.last = self.start;
    }
}

/// The walk along the decorated path.
#[derive(Debug, Clone)]
pub struct Walker {
    objs: Vec<Obj>,
    pos: usize,
    next: Obj,
    pub seg: SegInfo,
    pub next_kind: Kind,
    pub is_close: bool,
    /// the input segment before the current one was a moveto
    pub prev_is_moveto: bool,
    pub seg_completed: f64,
    pub seg_remaining: f64,
    pub time: f64,
    pub angle: f64,
    pub start_angle: f64,
    pub end_angle: f64,
    pub angle_to_next: f64,
    /// `\pgf@decorate@totalpathlength` and `\pgf@decorated@remainingdistance`
    pub total: f64,
    pub rem_total: f64,
    /// `\pgf@decorate@currentpathlength`: the length of the section being decorated
    pub section: f64,
    /// `\pgfdecoratedcompleteddistance`, `\pgfdecoratedremainingdistance`
    pub completed: f64,
    pub remaining: f64,
    pub total_first: Point,
    pub total_last: Point,
    pub path_first: Point,
    pub path_last: Point,
    pub out: Out,
}

impl Walker {
    /// Starts the walk (`\pgf@decoration@endenv`); `existing_last` is the current point before.
    pub fn new(parsed: Parsed, existing_last: Point) -> Walker {
        let mut w = Walker {
            objs: parsed.objs,
            pos: 0,
            next: Obj::End,
            seg: SegInfo::default(),
            next_kind: Kind::Last,
            is_close: false,
            prev_is_moveto: false,
            seg_completed: 0.0,
            seg_remaining: 0.0,
            time: 0.0,
            angle: 0.0,
            start_angle: 0.0,
            end_angle: 0.0,
            angle_to_next: 0.0,
            total: parsed.total,
            rem_total: parsed.total,
            section: parsed.total,
            completed: 0.0,
            remaining: parsed.total,
            total_first: parsed.first,
            total_last: parsed.last_nonmove,
            path_first: parsed.first,
            path_last: parsed.last_nonmove,
            out: Out { segs: Vec::new(), last: existing_last, start: existing_last },
        };
        w.get_next();
        w.process_next();
        w
    }

    pub fn exhausted(&self) -> bool {
        self.pos >= self.objs.len()
    }

    fn get_next(&mut self) {
        if let Some(o) = self.objs.get(self.pos) {
            self.next = *o;
            self.pos += 1;
        }
    }

    /// `\pgf@decorate@processnextinputsegmentobject`
    fn process_next(&mut self) {
        self.prev_is_moveto = false;
        let mut cur = self.next;
        self.get_next();
        self.is_close = matches!(cur, Obj::Close { .. });
        self.seg = self.seg.after(&cur);
        if self.seg.kind == Kind::MoveTo {
            self.out.move_to(self.seg.first);
            self.prev_is_moveto = true;
            cur = self.next;
            self.get_next();
            self.is_close = matches!(cur, Obj::Close { .. });
            self.seg = self.seg.after(&cur);
        }
        if self.seg.len.abs() < EPS {
            self.seg.len = 0.05;
        }
        self.seg_remaining = self.seg.len;
        self.seg_completed = 0.0;
        self.time = 0.0;
        let s = self.seg;
        if s.kind == Kind::CurveTo {
            self.angle = angle_between(s.first, s.sa);
            self.start_angle = self.angle;
            self.end_angle = angle_between(s.sb, s.last);
        } else {
            self.angle = angle_between(s.first, s.last);
            self.start_angle = self.angle;
            self.end_angle = self.angle;
        }
        // the angle to the next input segment (a moveto next: the current one again)
        let mut n = s.after(&self.next);
        self.next_kind = n.kind;
        if n.kind == Kind::MoveTo {
            n = n.after(&cur);
        }
        let a = if n.kind == Kind::CurveTo { angle_between(n.first, n.sa) } else { angle_between(n.first, n.last) };
        self.angle_to_next = a - self.angle;
    }

    /// `\pgf@decorate@movealongpath`
    pub fn move_along_path(&mut self, d: f64) {
        let d = trunc_sp(d);
        let mut dtm = d;
        self.completed += d;
        self.remaining -= d;
        // (at most so many input segments: a guard)
        for _ in 0..1_000_000 {
            self.seg_completed += dtm;
            self.seg_remaining -= dtm;
            if self.seg_remaining > EPS {
                self.move_along_input_segment(dtm);
                if self.time > 1.0 {
                    let t = self.time;
                    self.process_next();
                    self.time = t - 1.0;
                    if self.exhausted() {
                        self.remaining = 0.0;
                    }
                }
                return;
            }
            dtm = -self.seg_remaining;
            self.process_next();
            if self.exhausted() {
                self.remaining = 0.0;
                return;
            }
        }
    }

    /// `\pgf@decorate@movealonginputsegment`: on a line nothing moves (the transformation uses the
    /// completed distance); on a curve the time advances by pgf's bisection.
    pub fn move_along_input_segment(&mut self, d: f64) {
        if !self.seg.curve || d.abs() < SP / 2.0 {
            return;
        }
        let (t, angle) = self.curve_time_after(d);
        self.time = t;
        self.angle = angle;
    }

    fn curve_time_after(&self, d: f64) -> (f64, f64) {
        let s = self.seg;
        let at = |t: f64| curve_at(t, s.first, s.sa, s.sb, s.last);
        let mut prev = at(self.time).0;
        let mut xb = (self.time * 65536.0).round() as i64;
        let mut dt: i64 = if self.section < 128.0 {
            2048
        } else if self.section < 512.0 {
            1024
        } else if self.section < 2048.0 {
            256
        } else {
            64
        };
        let mut count: i64 = 1;
        let mut acc = 0.0;
        let mut guard = 0usize;
        loop {
            xb += count * dt;
            let p = at(xb as f64 / 65536.0).0;
            acc += count as f64 * p.minus(prev).norm();
            prev = p;
            if (count > 0 && acc > d) || (count < 0 && acc < d) {
                count = -count;
                dt /= 2;
            }
            guard += 1;
            if dt == 0 || guard > 2_000_000 {
                break;
            }
        }
        let t = xb as f64 / 65536.0;
        let (_, tangent) = at(t);
        (t, angle_between(Point::default(), tangent))
    }

    /// The transformation to the current position (`\pgf@decorate@transformtoinputsegment`).
    pub fn transform(&self) -> Transform {
        let s = self.seg;
        if s.curve {
            let (p, tangent) = curve_at(self.time, s.first, s.sa, s.sb, s.last);
            let u = normalised(tangent);
            Transform { a: u.x, b: u.y, c: -u.y, d: u.x, e: p.x, f: p.y }
        } else {
            let u = normalised(s.last.minus(s.first));
            let p = s.first.plus(u.scale(self.seg_completed));
            let r = Transform::rotation(self.angle);
            Transform { e: p.x, f: p.y, ..r }
        }
    }

    /// Where the current section ends, when it does not end with the path (`\pgfpoint@decorated@pathlast`).
    pub fn point_ahead(&self, d: f64) -> Point {
        let mut w = self.clone();
        w.move_along_path(d);
        w.transform().apply(Point::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_and_walk() {
        let segs = [Segment::MoveTo(Point::new(0.0, 0.0)), Segment::LineTo(Point::new(10.0, 0.0)), Segment::LineTo(Point::new(10.0, 10.0))];
        let p = parse(&segs, Point::default());
        assert!((p.total - 20.0).abs() < 1e-9);
        let mut w = Walker::new(p, Point::default());
        assert_eq!(w.out.segs, vec![Segment::MoveTo(Point::new(0.0, 0.0))]);
        w.move_along_path(15.0);
        let q = w.transform().apply(Point::new(1.0, 0.0));
        assert!((q.x - 10.0).abs() < 1e-9 && (q.y - 6.0).abs() < 1e-9, "{q:?}");
        w.move_along_path(5.0);
        assert!(w.remaining.abs() < 1e-9);
    }

    #[test]
    fn walking_a_curve() {
        let k = 0.5523 * 100.0;
        let segs = [Segment::MoveTo(Point::new(100.0, 0.0)), Segment::CurveTo(Point::new(100.0, k), Point::new(k, 100.0), Point::new(0.0, 100.0))];
        let mut w = Walker::new(parse(&segs, Point::default()), Point::default());
        for i in 1..15 {
            w.move_along_path(10.0);
            let q = w.transform().apply(Point::default());
            let a = (i as f64 * 10.0 / 100.0).to_degrees();
            assert!((q.x - 100.0 * a.to_radians().cos()).abs() < 0.3 && (q.y - 100.0 * a.to_radians().sin()).abs() < 0.3, "{i}: {q:?}");
        }
    }

    #[test]
    fn curve_length_is_close_to_the_arc() {
        // a quarter circle of radius 100pt
        let k = 0.5523 * 100.0;
        let l = curve_length(Point::new(100.0, 0.0), Point::new(100.0, k), Point::new(k, 100.0), Point::new(0.0, 100.0));
        assert!((l - std::f64::consts::FRAC_PI_2 * 100.0).abs() < 0.1, "{l}");
    }
}
