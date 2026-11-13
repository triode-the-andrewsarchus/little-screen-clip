//! The geometry shared by the engine, the canvas and the driver: points, affine transforms, path
//! segments and typeset labels. Everything in centimeters, y pointing up, as in TikZ.

use crate::color::Color;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Point {
        Point { x, y }
    }
    pub fn plus(self, o: Point) -> Point {
        Point::new(self.x + o.x, self.y + o.y)
    }
    pub fn minus(self, o: Point) -> Point {
        Point::new(self.x - o.x, self.y - o.y)
    }
    pub fn scale(self, k: f64) -> Point {
        Point::new(self.x * k, self.y * k)
    }
    pub fn norm(self) -> f64 {
        self.x.hypot(self.y)
    }
    /// The point at fraction `t` of the way to `o`.
    pub fn lerp(self, o: Point, t: f64) -> Point {
        self.plus(o.minus(self).scale(t))
    }
}

/// Affine transform: x' = a·x + c·y + e, y' = b·x + d·y + f.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Default for Transform {
    fn default() -> Self {
        Transform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 }
    }
}

impl Transform {
    pub fn apply(&self, p: Point) -> Point {
        Point::new(self.a * p.x + self.c * p.y + self.e, self.b * p.x + self.d * p.y + self.f)
    }
    /// Applies only the linear part (for relative offsets).
    pub fn linear(&self, p: Point) -> Point {
        Point::new(self.a * p.x + self.c * p.y, self.b * p.x + self.d * p.y)
    }
    /// `self` after `o` (`o` is applied first).
    pub fn compose(&self, o: &Transform) -> Transform {
        Transform {
            a: self.a * o.a + self.c * o.b,
            b: self.b * o.a + self.d * o.b,
            c: self.a * o.c + self.c * o.d,
            d: self.b * o.c + self.d * o.d,
            e: self.a * o.e + self.c * o.f + self.e,
            f: self.b * o.e + self.d * o.f + self.f,
        }
    }
    pub fn scaling(sx: f64, sy: f64) -> Transform {
        Transform { a: sx, d: sy, ..Default::default() }
    }
    pub fn translation(dx: f64, dy: f64) -> Transform {
        Transform { e: dx, f: dy, ..Default::default() }
    }
    pub fn rotation(degrees: f64) -> Transform {
        let (s, c) = degrees.to_radians().sin_cos();
        Transform { a: c, b: s, c: -s, d: c, ..Default::default() }
    }
    /// Average scale factor (for circle radii under `scale`).
    pub fn factor(&self) -> f64 {
        (self.a * self.d - self.b * self.c).abs().sqrt()
    }
    /// The inverse transform; the identity when `self` is singular (a zero scale).
    pub fn inverse(&self) -> Transform {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-15 {
            return Transform::default();
        }
        let (a, b, c, d) = (self.d / det, -self.b / det, -self.c / det, self.a / det);
        Transform { a, b, c, d, e: -(a * self.e + c * self.f), f: -(b * self.e + d * self.f) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    MoveTo(Point),
    LineTo(Point),
    CurveTo(Point, Point, Point),
    Close,
}

/// A typeset label: mathra's boxes, one per line, and the font scale.
#[derive(Debug, Clone)]
pub struct Label {
    pub lines: Vec<mathra::layout::MathBox>,
    /// Centimeters per mathra font unit.
    pub scale: f64,
    pub color: Color,
    pub align: char,
    /// Distance between baselines, in cm.
    pub line_height: f64,
    /// Horizontal scale of the glyphs (the boxes' positions already carry it): TeX's fonts of
    /// other design sizes are wider or narrower than the one font labels are drawn with.
    pub stretch: f64,
    /// Space added after each line besides the baselines' distance (`\\[2pt]`), in cm; may be
    /// shorter than the lines.
    pub skips: Vec<f64>,
    /// The font its glyphs are of.
    pub font: mathra::font::MathFont,
    /// How far the label's baseline is below its first line's, in cm: TeX's box of several lines
    /// sits on its last line's baseline (or around the math axis, a `tabular`).
    pub lift: f64,
}

impl Label {
    pub fn width(&self) -> f64 {
        self.lines.iter().map(|c| c.width).fold(0.0f32, f32::max) as f64 * self.scale
    }
    pub fn height(&self) -> f64 {
        self.lines.first().map_or(0.0, |c| c.height as f64 * self.scale) + self.lift
    }
    pub fn depth(&self) -> f64 {
        let n = self.lines.len();
        if n == 0 {
            return 0.0;
        }
        self.lines[n - 1].depth as f64 * self.scale - self.baseline(n - 1)
    }
    /// Where line `k`'s baseline is, from the first's (cm, y up: 0 or less).
    pub fn baseline(&self, k: usize) -> f64 {
        self.lift - (k as f64 * self.line_height + self.skips.iter().take(k).sum::<f64>())
    }
}
