//! Nonlinear transformations: `\pgftransformnonlinear{code}`, where the code moves the point in
//! `\pgf@x`, `\pgf@y` (canvas pt), as pgf's nonlinear transformations module runs it on what is drawn:
//! a line becomes a curve with its controls at a third and two thirds, a curve is halved until its
//! controls are within `\pgftransformnonlinearflatness` (5 pt) of each other and of its ends, and then
//! its controls and end are moved. `\pgfapproximatenonlineartransformation` and `…translation` turn
//! it into the linear transformation (or the shift) it makes at the origin. The curvilinear library's
//! `\pgfsetcurvilinearbeziercurve` and `\pgfpointcurvilinearbezierorthogonal` are here too.

use crate::scene::{Point, Segment, Transform};
use std::rc::Rc;

use crate::tex::{Body, Macro, Meaning, Param, Token};
use crate::{Error, PT};

use super::Engine;

/// The registers pgf's point code works in: `\pgf@x`, `\pgf@y`, `\pgf@xa` … `\pgf@yc`.
const REGISTERS: &[(&str, u32)] = &[("pgf@x", 4050), ("pgf@y", 4051), ("pgf@xa", 4052), ("pgf@ya", 4053), ("pgf@xb", 4054), ("pgf@yb", 4055), ("pgf@xc", 4056), ("pgf@yc", 4057)];

/// How far apart (pt) a curve's controls may be before it is halved.
const FLATNESS: f64 = 5.0;

pub fn define(e: &mut Engine) {
    for (name, r) in REGISTERS {
        e.tex.define(name, Meaning::Dimen(*r));
    }
    // `\pgf@sys@tonumber\reg`, expandable: what `\the` gives, without its pt (whose letters `\the`
    // writes as other characters, so the delimiter is made of those)
    let pt = [Param::Tok(Token::other('p')), Param::Tok(Token::other('t'))];
    let without_pt = Macro { params: [Param::Arg(1)].into_iter().chain(pt).collect(), body: vec![Body::Arg(1)], optional: None, protected: false };
    e.tex.define("tikra@withoutpt", Meaning::Macro(Rc::new(without_pt)));
    e.tex.define_text(r"\def\pgf@sys@tonumber#1{\expandafter\tikra@withoutpt\the#1}");
}

impl Engine {
    /// The point `p` (canvas cm) moved by the nonlinear transformations in force.
    pub fn nonlinear_point(&mut self, p: Point) -> Result<Point, Error> {
        let codes = self.st.nonlinear.clone();
        let mut p = p;
        for code in codes.iter() {
            self.tex.dimens.insert(4050, (p.x / PT * 65536.0).round() as i64);
            self.tex.dimens.insert(4051, (p.y / PT * 65536.0).round() as i64);
            self.begin_group();
            let r = self.run_tokens(code.clone());
            let (x, y) = (self.tex.dimens.get(&4050).copied().unwrap_or(0), self.tex.dimens.get(&4051).copied().unwrap_or(0));
            self.end_group();
            r?;
            p = Point::new(x as f64 / 65536.0 * PT, y as f64 / 65536.0 * PT);
        }
        Ok(p)
    }

    /// A path (canvas cm) as the nonlinear transformations draw it.
    pub fn nonlinear_path(&mut self, segs: &[Segment]) -> Result<Vec<Segment>, Error> {
        let mut out = Vec::new();
        let (mut last, mut start) = (Point::default(), Point::default());
        for s in segs {
            match *s {
                Segment::MoveTo(p) => {
                    out.push(Segment::MoveTo(self.nonlinear_point(p)?));
                    (last, start) = (p, p);
                }
                Segment::LineTo(p) => {
                    self.nonlinear_line(last, p, &mut out)?;
                    last = p;
                }
                Segment::CurveTo(a, b, p) => {
                    self.nonlinear_curve([last, a, b, p], &mut out, 0)?;
                    last = p;
                }
                Segment::Close => {
                    if last.minus(start).norm() / PT > 0.01 {
                        self.nonlinear_line(last, start, &mut out)?;
                    }
                    out.push(Segment::Close);
                    last = start;
                }
            }
        }
        Ok(out)
    }

    fn nonlinear_line(&mut self, a: Point, c: Point, out: &mut Vec<Segment>) -> Result<(), Error> {
        let d = c.minus(a);
        if d.x.abs() / PT < 0.1 && d.y.abs() / PT < 0.1 {
            out.push(Segment::LineTo(self.nonlinear_point(c)?));
            return Ok(());
        }
        self.nonlinear_curve([a, a.lerp(c, 1.0 / 3.0), a.lerp(c, 2.0 / 3.0), c], out, 0)
    }

    fn nonlinear_curve(&mut self, q: [Point; 4], out: &mut Vec<Segment>, depth: usize) -> Result<(), Error> {
        let [a, b1, b2, c] = q;
        let far = |u: Point, v: Point| (u.x - v.x).abs() / PT > FLATNESS || (u.y - v.y).abs() / PT > FLATNESS;
        if depth < 16 && (far(b1, b2) || far(b1, a) || far(b2, c)) {
            // halved at t = 1/2 (de Casteljau)
            let (ab, bc, cd) = (a.lerp(b1, 0.5), b1.lerp(b2, 0.5), b2.lerp(c, 0.5));
            let (abc, bcd) = (ab.lerp(bc, 0.5), bc.lerp(cd, 0.5));
            let m = abc.lerp(bcd, 0.5);
            self.nonlinear_curve([a, ab, abc, m], out, depth + 1)?;
            return self.nonlinear_curve([m, bcd, cd, c], out, depth + 1);
        }
        let (x, y, z) = (self.nonlinear_point(b1)?, self.nonlinear_point(b2)?, self.nonlinear_point(c)?);
        out.push(Segment::CurveTo(x, y, z));
        Ok(())
    }

    /// `\pgftransformnonlinear{code}`.
    pub fn transform_nonlinear(&mut self) -> Result<(), Error> {
        let code = self.argument()?;
        std::rc::Rc::make_mut(&mut self.st.nonlinear).push(code);
        Ok(())
    }

    /// `\pgfapproximatenonlineartransformation` (`translation`: only the shift).
    pub fn approximate_nonlinear(&mut self, full: bool) -> Result<(), Error> {
        if self.st.nonlinear.is_empty() {
            return Ok(());
        }
        let t = self.st.transform;
        let o = self.nonlinear_point(t.apply(Point::default()))?;
        if full {
            let ex = self.nonlinear_point(t.apply(Point::new(PT, 0.0)))?.minus(o).scale(1.0 / PT);
            let ey = self.nonlinear_point(t.apply(Point::new(0.0, PT)))?.minus(o).scale(1.0 / PT);
            self.st.transform = Transform { a: ex.x, b: ex.y, c: ey.x, d: ey.y, e: o.x, f: o.y };
        } else {
            self.st.transform = Transform { e: o.x, f: o.y, ..t };
        }
        self.st.nonlinear = Default::default();
        Ok(())
    }

    /// `\pgfsetcurvilinearbeziercurve{p0}{p1}{p2}{p3}`.
    pub fn set_curvilinear(&mut self) -> Result<(), Error> {
        let t = self.st.transform;
        let mut p = [Point::default(); 4];
        for q in p.iter_mut() {
            *q = t.apply(self.point_arg()?);
        }
        self.st.curvilinear = Some(std::rc::Rc::new(super::arrows::Curvilinear::new(p)));
        Ok(())
    }

    /// `\pgfpointcurvilinearbezierorthogonal{distance}{offset}`: the point `distance` along the curve
    /// and `offset` to its left (as the picture's y axis is to the left of its x axis: the map keeps
    /// the orientation), into `\pgf@x`, `\pgf@y`.
    pub fn point_curvilinear(&mut self) -> Result<(), Error> {
        let (a, b) = (self.argument()?, self.argument()?);
        let (along, off) = (self.length_of(a)? / PT, self.length_of(b)? / PT);
        let Some(c) = self.st.curvilinear.clone() else { return Ok(()) };
        let t = c.time(along);
        let p = c.at(t);
        // the tangent of de Casteljau's last step, turned to the left
        let [q0, q1, q2, q3] = c.points;
        let (ab, bc, cd) = (q0.lerp(q1, t), q1.lerp(q2, t), q2.lerp(q3, t));
        let mut d = bc.lerp(cd, t).minus(ab.lerp(bc, t));
        if d.norm() < 1e-9 {
            d = q2.minus(q0);
        }
        let n = Point::new(-d.y, d.x).scale(1.0 / d.norm().max(1e-12));
        let r = p.plus(n.scale(off * PT));
        self.tex.dimens.insert(4050, (r.x / PT * 65536.0).round() as i64);
        self.tex.dimens.insert(4051, (r.y / PT * 65536.0).round() as i64);
        Ok(())
    }

    /// `\pgfpointcurvilinearbezierpolar{x}{y}`: the point as far along the curve as (x,y) is from
    /// the origin, turned about the curve's start by the angle of (x,y) (of (−x,−y) when x < 0,
    /// the distance then counting backwards), into `\pgf@x`, `\pgf@y`.
    pub fn point_curvilinear_polar(&mut self) -> Result<(), Error> {
        let (a, b) = (self.argument()?, self.argument()?);
        let (x, y) = (self.length_of(a)? / PT, self.length_of(b)? / PT);
        let Some(c) = self.st.curvilinear.clone() else { return Ok(()) };
        let mut dir = if x == 0.0 && y == 0.0 { Point::new(1.0, 0.0) } else { Point::new(x, y).scale(1.0 / x.hypot(y)) };
        let mut d = x.hypot(y);
        if x < 0.0 {
            dir = dir.scale(-1.0);
            d = -d;
        }
        let p = c.at(c.time(d));
        let start = c.points[0];
        let v = p.minus(start);
        let r = start.plus(Point::new(dir.x * v.x - dir.y * v.y, dir.y * v.x + dir.x * v.y));
        self.tex.dimens.insert(4050, (r.x / PT * 65536.0).round() as i64);
        self.tex.dimens.insert(4051, (r.y / PT * 65536.0).round() as i64);
        Ok(())
    }

    /// `\pgfmathsincos@{angle}`: `\pgfmathresultx` its cosine, `\pgfmathresulty` its sine.
    pub fn math_sincos(&mut self) -> Result<(), Error> {
        let a = self.argument()?;
        let a = self.number_of(a)?;
        let (s, c) = a.to_radians().sin_cos();
        self.tex.set_meaning(&Token::cs("pgfmathresultx"), super::macro_from(&super::pgf_number(c)), false);
        self.tex.set_meaning(&Token::cs("pgfmathresulty"), super::macro_from(&super::pgf_number(s)), false);
        Ok(())
    }
}
