//! A 3d axis as pgfplots draws it (`\pgfplots@draw@axis` in 3d): face by face of the axis box,
//! on "oriented surfaces".
//!
//! pgfplots names the parts of the box by three-character strings, one character per axis:
//! `v` varies, `0` is the lower limit, `1` the upper one and `2` the zero of a centered axis line.
//! `v00` is the x axis line at the lower y and z limits, `vv0` the face at the lower z limit.
//! The view direction says which faces are in front (`\pgfplots@decide@which@figure@surfaces@are@drawn`):
//! only lines that touch a face in the background are drawn, grid lines only on faces in the
//! background, and tick labels go on lines of the projected box's outline
//! (`\pgfplots@init@ticklabelaxisspec`).

use crate::scene::{Point, Segment};
use crate::Error;

use super::draw::{axis_char, line_shift, minor_ticks, number_list, tick_labels_at, tick_offset, unit, visible, Frame, Setup};
use super::keys;
use super::Engine;

/// A part of the axis box: a line or a face (see the module documentation).
pub type Part = [u8; 3];

/// `\pgfplotsgetnormalforcurrentview`: the view direction, the kernel of the projection, with
/// pgfplots' choice of sign. The unit vectors are taken without the reversal of axes.
pub fn view_dir(ev: &[Point; 3], dir: [bool; 3]) -> [f64; 3] {
    let s = |k: usize| if dir[k] { -1.0 } else { 1.0 };
    let (x, y, z) = (ev[0].scale(s(0)), ev[1].scale(s(1)), ev[2].scale(s(2)));
    let zero = |p: Point| p.x.abs() < 1e-12 && p.y.abs() < 1e-12;
    if zero(x) {
        return [-1.0, 0.0, 0.0];
    }
    if zero(y) {
        return [0.0, 1.0, 0.0];
    }
    if x.y.abs() < 1e-12 && y.y.abs() < 1e-12 {
        // one row: n_z = 0
        let ny = 1.0;
        let nx = -ny * y.x / x.x;
        let scale = if (z.y > 0.0 && x.x < 0.0) || (z.y <= 0.0 && x.x > 0.0) { -1.0 } else { 1.0 };
        return normalised([nx * scale, ny * scale, 0.0]);
    }
    // n_x x + n_y y = z with n_z = -1
    let nz = -1.0;
    let det = x.x * y.y - y.x * x.y;
    if det.abs() < 1e-12 {
        return [0.0, 0.0, 1.0];
    }
    let (bx, by) = (-nz * z.x, -nz * z.y);
    let nx = (bx * y.y - y.x * by) / det;
    let ny = (x.x * by - bx * x.y) / det;
    let mut scale = 1.0;
    if x.x > 0.0 {
        if y.x >= 0.0 && x.y >= 0.0 && y.y < 0.0 {
            scale = -1.0;
        }
    } else if x.x < 0.0 && y.x <= 0.0 && x.y <= 0.0 && y.y > 0.0 {
        scale = -1.0;
    }
    if x.y > 0.0 {
        if y.y >= 0.0 && x.x <= 0.0 && y.x > 0.0 {
            scale = -1.0;
        }
    } else if x.y < 0.0 && y.y <= 0.0 && x.x >= 0.0 && y.x < 0.0 {
        scale = -1.0;
    }
    normalised([nx * scale, ny * scale, nz * scale])
}

/// (`v` is never zero: one of its components is ±1)
fn normalised(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    v.map(|c| c / l)
}

/// Which faces are in front: `[axis][lower, upper]`.
pub fn foreground(n: [f64; 3], dir: [bool; 3]) -> [[bool; 2]; 3] {
    [0, 1, 2].map(|k| {
        let positive = if dir[k] { n[k] < 0.0 } else { n[k] > 0.0 };
        // a view along +x sees the face at the lower x limit from the front
        if positive {
            [true, false]
        } else {
            [false, true]
        }
    })
}

/// `\pgfplotsifaxissurfaceisforeground`: faces at a centered line count as foreground.
fn is_fg(fg: &[[bool; 2]; 3], s: Part) -> bool {
    let k = (0..3).find(|&k| s[k] != b'v').expect("a face has an axis fixed");
    match s[k] {
        b'0' => fg[k][0],
        b'1' => fg[k][1],
        _ => true,
    }
}

/// `\pgfplotsgetadjacentsurfsforaxisline`: the two faces a line belongs to.
fn adjacent(l: Part) -> (Part, Part) {
    let v = b'v';
    if l[0] == v {
        ([v, v, l[2]], [v, l[1], v])
    } else if l[1] == v {
        ([v, v, l[2]], [l[0], v, v])
    } else {
        ([v, l[1], v], [l[0], v, v])
    }
}

/// `\pgfplotsifaxislineisonconvexhull`: one face in front and one behind.
fn on_hull(fg: &[[bool; 2]; 3], l: Part) -> bool {
    let (a, b) = adjacent(l);
    is_fg(fg, a) != is_fg(fg, b)
}

/// `axis x line*`: 0 box, 1 bottom/left, 2 middle, 3 top/right, 4 none.
fn lines_num(line: &str) -> u8 {
    match line {
        "box" => 0,
        "low" => 1,
        "center" => 2,
        "high" => 3,
        _ => 4,
    }
}

/// `xtick pos`: 0 both, 1 lower, 2 center, 3 upper, 4 none.
fn tick_pos_num(pos: &str) -> u8 {
    match pos {
        "left" | "bottom" | "lower" => 1,
        "center" | "middle" => 2,
        "right" | "top" | "upper" => 3,
        "none" => 4,
        _ => 0,
    }
}

/// Sets the faces in front and the tick label lines of a 3d axis (`\pgfplots@init@ticklabelaxisspec`).
pub fn prepare(e: &mut Engine, f: &mut Frame, dir: [bool; 3], lines: &[String; 3]) {
    let n = view_dir(&f.e, dir);
    f.fg = foreground(n, dir);
    let axis_vec = |k: usize| f.e[k].scale(f.max[k] - f.min[k]);
    let center = axis_vec(0).plus(axis_vec(1)).plus(axis_vec(2)).scale(0.5);
    for (a, line) in lines.iter().enumerate() {
        let c = axis_char(a);
        let num = lines_num(line);
        let tickpos = tick_pos_num(&e.pv(&format!("{c}tick pos")));
        // 'l' or 'u'
        let pos = if num != 0 {
            if tickpos == 3 {
                b'u'
            } else {
                b'l'
            }
        } else {
            match e.pv(&format!("{c}ticklabel pos")).as_str() {
                "upper" | "right" | "top" => b'u',
                "lower" | "left" | "bottom" => b'l',
                _ => {
                    if tickpos == 3 {
                        b'u'
                    } else {
                        b'l'
                    }
                }
            }
        };
        let to3 = |i: u8, j: u8| -> Part {
            match a {
                0 => [b'v', i, j],
                1 => [i, b'v', j],
                _ => [i, j, b'v'],
            }
        };
        // x and y compare the projected y; z the projected x
        let coord = |p: Point| if a == 2 { p.x } else { p.y };
        let first = if a == 0 { 1 } else { 0 };
        let mut spec;
        if on_hull(&f.fg, to3(b'0', b'0')) {
            spec = to3(b'0', b'0');
            let p = axis_vec(a).scale(0.5);
            let below = coord(p) < coord(center);
            if (below && pos == b'u') || (!below && pos == b'l') {
                spec = to3(b'1', b'1');
            }
        } else {
            spec = to3(b'1', b'0');
            let p = axis_vec(first).plus(axis_vec(a).scale(0.5));
            let below = coord(p) < coord(center);
            if (below && pos == b'u') || (!below && pos == b'l') {
                spec = to3(b'0', b'1');
            }
        }
        f.box_line[a] = spec;
        f.label_line[a] = if num == 2 { to3(b'2', b'2') } else { spec };
    }
}

/// `\pgfplotspointouternormalvectorofaxis`: the unit vector out of the box at a line.
pub fn outer_normal(ev: &[Point; 3], line: Part, box_line: Part) -> Point {
    let Some(v) = (0..3).find(|&k| line[k] == b'v') else { return Point::default() };
    let mut sum = Point::default();
    for k in (0..3).filter(|&k| k != v) {
        let ch = if line[k] == b'2' { box_line[k] } else { line[k] };
        let sign = if ch == b'1' { 1.0 } else { -1.0 };
        if ev[k].norm() > 1e-12 {
            sum = sum.plus(unit(ev[k]).scale(sign));
        }
    }
    if sum.norm() > 1e-12 {
        unit(sum)
    } else {
        sum
    }
}

/// An oriented surface: tick positions go along `a`, tick lines along `b`, `n` is fixed.
#[derive(Clone, Copy)]
struct Surf {
    a: usize,
    b: usize,
    n: usize,
    value: f64,
    sym: u8,
}

impl Surf {
    fn at(&self, f: &Frame, av: f64, bv: f64) -> Point {
        let mut l = [0.0; 3];
        l[self.a] = av;
        l[self.b] = bv;
        l[self.n] = self.value;
        f.at(l)
    }

    /// `\pgfplotspointonorientedsurfacespec`: `ab0` and the like.
    fn spec(&self) -> Part {
        let mut p = [0; 3];
        p[self.a] = b'a';
        p[self.b] = b'b';
        p[self.n] = self.sym;
        p
    }

    /// The line of this surface with `b` at `bval` (`\pgfplotspointonorientedsurfaceabtolinespec v#1`).
    fn line(&self, bval: u8) -> Part {
        let mut p = [0; 3];
        p[self.a] = b'v';
        p[self.b] = bval;
        p[self.n] = self.sym;
        p
    }
}

/// `\pgfplotspointonorientedsurfaceabmatchaxisline`: the `b` character of `line` if the line lies
/// on the surface.
fn match_line(line: Part, surf: Part) -> Option<u8> {
    let bpos = (0..3).find(|&k| surf[k] == b'b');
    let mut r = bpos.map_or(b'v', |k| line[k]);
    for k in 0..3 {
        let (c, s) = (line[k], surf[k]);
        let ok = match c {
            b'v' => matches!(s, b'a' | b'b' | b'v'),
            b'0' => s != b'1',
            b'1' => s != b'0',
            // b'2' (a line is made of v, 0, 1 and 2 only)
            _ => s != b'0' && s != b'1',
        };
        if !ok {
            r = 0;
        }
    }
    (r != 0).then_some(r)
}

struct Ctx<'a> {
    s: &'a Setup,
    /// `3d box=complete`'s second pass over the faces in front
    post: bool,
    /// `3d box=complete*`: with grid lines
    post_grid: bool,
    nums: [u8; 3],
    tickpos: [u8; 3],
    dir: [bool; 3],
}

impl Ctx<'_> {
    fn f(&self) -> &Frame {
        &self.s.frame
    }

    /// `\pgfplots@ifaxisline@B@onorientedsurf@should@be@drawn@`: from the faces alone.
    fn drawn_faces(&self, surf: &Surf, bval: u8) -> bool {
        let (s1, s2) = adjacent(surf.line(bval));
        let both = is_fg(&self.f().fg, s1) && is_fg(&self.f().fg, s2);
        if self.post {
            both
        } else {
            !both
        }
    }

    /// `\pgfplots@ifaxisline@B@onorientedsurf@should@be@drawn`: also by `axis lines`.
    fn drawn(&self, surf: &Surf, bval: u8) -> bool {
        self.drawn_faces(surf, bval) && (matches!(self.nums[surf.a], 0 | 2) || surf.line(bval) == self.f().label_line[surf.a])
    }

    fn grid_drawn(&self, surf: &Surf) -> bool {
        if self.post {
            return self.post_grid;
        }
        self.drawn_faces(surf, b'0') && self.drawn_faces(surf, b'1')
    }

    fn grid(&self, e: &mut Engine, surf: &Surf, list: &[f64], extra: bool) -> Result<(), Error> {
        if !self.grid_drawn(surf) {
            return Ok(());
        }
        let f = self.f();
        let c = axis_char(surf.a);
        let line = |t: f64| vec![Segment::MoveTo(surf.at(f, t, f.min[surf.b])), Segment::LineTo(surf.at(f, t, f.max[surf.b]))];
        if keys::flag(e, &format!("/pgfplots/{c}minorgrids")) && !extra {
            let segs: Vec<Segment> = minor_ticks(e, self.s, surf.a, list).into_iter().flat_map(line).collect();
            e.draw_with(&format!("/pgfplots/every axis grid,/pgfplots/every minor grid,/pgfplots/every axis {c} grid,/pgfplots/every minor {c} grid"), segs)?;
        }
        if keys::flag(e, &format!("/pgfplots/{c}majorgrids")) {
            let segs: Vec<Segment> = visible(f, surf.a, list).into_iter().flat_map(|(_, t)| line(t)).collect();
            e.draw_with(&format!("/pgfplots/every axis grid,/pgfplots/every major grid,/pgfplots/every axis {c} grid,/pgfplots/every major {c} grid"), segs)?;
        }
        Ok(())
    }

    /// `\pgfplots@drawticklines@onorientedsurf`
    fn ticks(&self, e: &mut Engine, surf: &Surf, list: &[f64], extra: bool) -> Result<(), Error> {
        let f = self.f();
        let (a, b) = (surf.a, surf.b);
        let c = axis_char(a);
        let (mut lower, mut upper) = if self.nums[a] == 2 { (true, false) } else { (self.drawn(surf, b'0'), self.drawn(surf, b'1')) };
        let mut pos = self.tickpos[a];
        if self.dir[b] {
            pos = match pos {
                1 => 3,
                3 => 1,
                p => p,
            };
        }
        match pos {
            0 => {}
            3 => lower = false,
            _ => upper = false,
        }
        if pos == 4 {
            return Ok(());
        }
        let lower_b = if pos == 2 { f.zero[b] } else { f.min[b] };
        let ub = unit(f.e[b]);
        let align = e.pv(&format!("{c}tick align"));
        let mut kinds = Vec::new();
        if keys::flag(e, &format!("/pgfplots/{c}minorticks")) && !extra {
            let m = minor_ticks(e, self.s, a, list);
            if !m.is_empty() {
                kinds.push(("minor", m, keys::length(e, "/pgfplots/minor tick length").unwrap_or(0.1)));
            }
        }
        if keys::flag(e, &format!("/pgfplots/{c}majorticks")) {
            let m: Vec<f64> = visible(f, a, list).into_iter().map(|(_, t)| t).collect();
            kinds.push(("major", m, keys::length(e, "/pgfplots/major tick length").unwrap_or(0.15)));
        }
        let (sh0, sh1) = if self.nums[a] == 2 { (Point::default(), Point::default()) } else { (self.line_shift(e, surf, b'0'), self.line_shift(e, surf, b'1')) };
        for (kind, ticks, len) in kinds {
            let off = match align.as_str() {
                "outside" => len,
                "center" => len / 2.0,
                _ => 0.0,
            };
            let mut segs = Vec::new();
            for t in ticks {
                if lower {
                    let p = surf.at(f, t, lower_b).plus(sh0);
                    segs.push(Segment::MoveTo(p.plus(ub.scale(-off))));
                    segs.push(Segment::LineTo(p.plus(ub.scale(len - off))));
                }
                if upper {
                    let p = surf.at(f, t, f.max[b]).plus(sh1);
                    segs.push(Segment::MoveTo(p.plus(ub.scale(off))));
                    segs.push(Segment::LineTo(p.plus(ub.scale(off - len))));
                }
            }
            if !segs.is_empty() {
                e.draw_with(&format!("/pgfplots/every tick,/pgfplots/every {kind} tick,/pgfplots/every {c} tick,/pgfplots/every {kind} {c} tick"), segs)?;
            }
        }
        Ok(())
    }

    /// `axis x line shift=10pt`: where the line of the surface with `b` at `bval` moves, with its
    /// ticks and labels, along its outer normal (`\pgfplots@transformshift@along@outer@normal…`).
    fn line_shift(&self, e: &mut Engine, surf: &Surf, bval: u8) -> Point {
        let f = self.f();
        let d = line_shift(e, f, surf.a);
        if d == 0.0 {
            return Point::default();
        }
        outer_normal(&f.e, surf.line(bval), f.box_line[surf.a]).scale(d)
    }

    /// The outer lines (`\pgfplots@drawaxis@outerlines@separate@onorientedsurf`) or the line
    /// through the zero (`…@innerlines@…`).
    fn lines(&self, e: &mut Engine, surf: &Surf) -> Result<(), Error> {
        let f = self.f();
        let (a, b) = (surf.a, surf.b);
        let c = axis_char(a);
        if self.nums[a] == 2 {
            let mut inner = *surf;
            inner.value = f.zero[surf.n];
            let segs = vec![Segment::MoveTo(inner.at(f, f.min[a], f.zero[b])), Segment::LineTo(inner.at(f, f.max[a], f.zero[b]))];
            return e.draw_with(&format!("/pgfplots/every inner {c} axis line"), segs);
        }
        for (bval, bv) in [(b'0', f.min[b]), (b'1', f.max[b])] {
            if self.drawn(surf, bval) {
                let sh = self.line_shift(e, surf, bval);
                let segs = vec![Segment::MoveTo(surf.at(f, f.min[a], bv).plus(sh)), Segment::LineTo(surf.at(f, f.max[a], bv).plus(sh))];
                e.draw_with(&format!("/pgfplots/every outer {c} axis line"), segs)?;
            }
        }
        Ok(())
    }

    /// `\pgfplots@drawticklabels@onorientedsurf`
    fn labels(&self, e: &mut Engine, surf: &Surf, list: &[f64], extra: bool) -> Result<(), Error> {
        if self.post {
            return Ok(());
        }
        let f = self.s.frame.clone();
        let (a, b) = (surf.a, surf.b);
        let Some(side) = match_line(f.label_line[a], surf.spec()) else { return Ok(()) };
        if !keys::flag(e, &format!("/pgfplots/{}majorticks", axis_char(a))) {
            return Ok(());
        }
        let off = tick_offset(e, a);
        let (origin, offset) = if self.nums[a] == 2 {
            (f.zero[b], if self.dir[b] { off } else { -off })
        } else {
            // (the side is 0 or 1: a centered line has its own case above)
            match side {
                b'0' => (f.min[b], -off),
                _ => (f.max[b], off),
            }
        };
        let moved = if self.nums[a] == 2 || !matches!(side, b'0' | b'1') { Point::default() } else { self.line_shift(e, surf, side) };
        let shift = unit(f.e[b]).scale(offset);
        let s = *surf;
        tick_labels_at(e, self.s, a, list, extra, &|t| s.at(&f, t, origin).plus(moved), shift, off)
    }

    /// `extra x ticks` on the surface, with their labels or without.
    fn extra(&self, e: &mut Engine, surf: &Surf, labels: bool) -> Result<(), Error> {
        let a = surf.a;
        let c = axis_char(a);
        let list = crate::keys::value(&e.tex, &format!("/pgfplots/extra {c} ticks")).unwrap_or_default();
        if crate::keys::trim(&list).is_empty() {
            return Ok(());
        }
        let f = self.f();
        let v: Vec<f64> = number_list(e, list)?.into_iter().map(|x| f.logical(a, x)).collect();
        e.begin_group();
        let r = (|| {
            e.st.key_path = "/pgfplots".into();
            keys::run(e, &format!("every extra {c} tick"))?;
            self.grid(e, surf, &v, true)?;
            self.ticks(e, surf, &v, true)?;
            if labels {
                self.labels(e, surf, &v, true)?;
            }
            Ok(())
        })();
        e.end_group();
        r
    }
}

/// The 3d axis box: its lines, grid lines, ticks and tick labels. With `post`, the pass of
/// `3d box=complete` over the faces in front (`\pgfplots@draw@axis@post`).
pub(super) fn draw(e: &mut Engine, s: &Setup, post: bool) -> Result<(), Error> {
    let f = s.frame.clone();
    let nums = s.line.clone().map(|l| lines_num(&l));
    let tickpos = [0, 1, 2].map(|a| tick_pos_num(&e.pv(&format!("{}tick pos", axis_char(a)))));
    let dir = [e.pv("x dir") == "reverse", e.pv("y dir") == "reverse", e.pv("z dir") == "reverse"];
    let choice = e.pv("3d box");
    if post && (choice == "background" || nums.iter().any(|n| *n != 0)) {
        return Ok(());
    }
    let ctx = Ctx { s, post, post_grid: choice == "complete*", nums, tickpos, dir };
    let hide = s.hide;
    let ticks = &s.ticks;
    // the faces of each fixed axis: lower and upper limit, or the zero of a centered line
    let faces = |k: usize| -> Vec<(f64, u8)> {
        if nums[k] == 2 {
            vec![(f.zero[k], b'2')]
        } else {
            vec![(f.min[k], b'0'), (f.max[k], b'1')]
        }
    };
    let in_front = |k: usize, sym: u8| -> bool {
        let mut p = [b'v'; 3];
        p[k] = sym;
        is_fg(&f.fg, p)
    };
    if post {
        e.begin_group();
        e.st.key_path = "/pgfplots".into();
        keys::run(e, "every 3d box foreground")?;
    }
    let r = (|| -> Result<(), Error> {
        // z fixed: x and y in turn (`\pgfplots@draw@axis@insurface@symmetric xyz`)
        for (value, sym) in faces(2) {
            if post && !in_front(2, sym) {
                continue;
            }
            let sx = Surf { a: 0, b: 1, n: 2, value, sym };
            let sy = Surf { a: 1, b: 0, n: 2, value, sym };
            for (surf, a) in [(&sx, 0), (&sy, 1)] {
                if !hide[a] {
                    ctx.grid(e, surf, &ticks[a], false)?;
                }
            }
            for (surf, a) in [(&sx, 0), (&sy, 1)] {
                if !hide[a] {
                    ctx.ticks(e, surf, &ticks[a], false)?;
                }
            }
            for (surf, a) in [(&sx, 0), (&sy, 1)] {
                if !hide[a] {
                    ctx.lines(e, surf)?;
                }
            }
            for (surf, a) in [(&sx, 0), (&sy, 1)] {
                if !hide[a] {
                    ctx.labels(e, surf, &ticks[a], false)?;
                }
            }
            for (surf, a) in [(&sx, 0), (&sy, 1)] {
                if !hide[a] {
                    ctx.extra(e, surf, true)?;
                }
            }
        }
        // x fixed: y's ticks and grid along z, then all of z along y
        for (value, sym) in faces(0) {
            if post && !in_front(0, sym) {
                continue;
            }
            let sy = Surf { a: 1, b: 2, n: 0, value, sym };
            if !hide[1] {
                ctx.grid(e, &sy, &ticks[1], false)?;
                ctx.ticks(e, &sy, &ticks[1], false)?;
                ctx.extra(e, &sy, false)?;
            }
            let sz = Surf { a: 2, b: 1, n: 0, value, sym };
            if !hide[2] {
                ctx.grid(e, &sz, &ticks[2], false)?;
                ctx.ticks(e, &sz, &ticks[2], false)?;
                ctx.lines(e, &sz)?;
                ctx.labels(e, &sz, &ticks[2], false)?;
                ctx.extra(e, &sz, true)?;
            }
        }
        // y fixed: x's and z's ticks and grid
        for (value, sym) in faces(1) {
            if post && !in_front(1, sym) {
                continue;
            }
            for (a, b) in [(0, 2), (2, 0)] {
                let surf = Surf { a, b, n: 1, value, sym };
                if !hide[a] {
                    ctx.grid(e, &surf, &ticks[a], false)?;
                    ctx.ticks(e, &surf, &ticks[a], false)?;
                    ctx.extra(e, &surf, false)?;
                }
            }
        }
        Ok(())
    })();
    if post {
        e.end_group();
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_view_faces() {
        // view {25}{30}: x to the right and down, y to the right and up, z up
        let (saz, caz) = 25f64.to_radians().sin_cos();
        let (sel, cel) = (-30f64).to_radians().sin_cos();
        let ev = [Point::new(caz, saz * sel), Point::new(saz, -sel * caz), Point::new(0.0, cel)];
        let n = view_dir(&ev, [false; 3]);
        // pgfplots' n = (-sin az cos el, cos az cos el, sin(-el)), up to its length
        let want = [-saz * cel, caz * cel, sel];
        let l = (want[0] * want[0] + want[1] * want[1] + want[2] * want[2]).sqrt();
        for k in 0..3 {
            assert!((n[k] - want[k] / l).abs() < 1e-9, "{n:?}");
        }
        let fg = foreground(n, [false; 3]);
        // the faces at the upper x, the lower y and the upper z limits are in front
        assert_eq!(fg, [[false, true], [true, false], [false, true]]);
        // the bottom front edge holds x's tick labels
        assert!(on_hull(&fg, *b"v00"));
        assert!(!on_hull(&fg, *b"v10"));
        assert_eq!(match_line(*b"v00", *b"ab0"), Some(b'0'));
        assert_eq!(match_line(*b"v00", *b"ab1"), None);
    }
}
