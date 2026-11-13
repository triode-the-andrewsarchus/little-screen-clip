//! The patch types of `\addplot [patch]` (`patch type=…`, the `patchplots` library): their
//! points, their outline, the surface between them, and `patch refines` and `patch to
//! triangles`.
//!
//! Points are in the axis' logical coordinates (where the canvas is affine), so a curve's Bézier
//! form there is the canvas' one. Higher order patches interpolate their points (`biquadratic`,
//! `bicubic`, `triangle quadr`, the splines) or are Bézier patches (`coons`, `tensor bezier`); the
//! color of a shaded patch goes bilinearly between the colors of its corners (the shadings of
//! type 6 and 7 that pgfplots writes), and a refined patch's points and colors come from its
//! parent's interpolation.

/// A point: x, y, z in logical coordinates.
pub type P3 = [f64; 3];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// One dimensional: `line`, `quadratic spline` (ends, middle), `cubic spline` (ends, points at
    /// 1/3 and 2/3), `bezier spline` (ends, control points).
    Line,
    QuadSpline,
    CubicSpline,
    BezierSpline,
    /// Flat: `triangle`, `rectangle` (two triangles when shaded), `bilinear`, `polygon`.
    Triangle,
    Rectangle,
    Bilinear,
    Polygon(usize),
    /// `triangle quadr`: corners A, B, C, then the middles of AB, BC, CA.
    QuadTriangle,
    /// `biquadratic`: corners A, B, C, D, the middles of AB, BC, CD, DA, the center.
    Biquadratic,
    /// `bicubic`: 4 × 4 points row by row (x varies), at 0, 1/3, 2/3, 1.
    Bicubic,
    /// `coons`: 12 Bézier points around (corners 0, 3, 6, 9); `tensor bezier` 4 more inside.
    Coons,
    Tensor,
}

impl Kind {
    /// `patch type`, with `vertex count` for polygons.
    pub fn from_name(name: &str, vertex_count: usize) -> Kind {
        match name.trim() {
            "line" => Kind::Line,
            "quadratic spline" => Kind::QuadSpline,
            "cubic spline" => Kind::CubicSpline,
            "bezier spline" => Kind::BezierSpline,
            "rectangle" => Kind::Rectangle,
            "bilinear" => Kind::Bilinear,
            "polygon" => Kind::Polygon(vertex_count.max(3)),
            "triangle quadr" | "quadratic triangle" => Kind::QuadTriangle,
            "biquadratic" => Kind::Biquadratic,
            "bicubic" => Kind::Bicubic,
            "coons" => Kind::Coons,
            "tensor bezier" => Kind::Tensor,
            _ => Kind::Triangle,
        }
    }

    /// The number of points of one patch.
    pub fn size(self) -> usize {
        match self {
            Kind::Line => 2,
            Kind::QuadSpline | Kind::Triangle => 3,
            Kind::CubicSpline | Kind::BezierSpline | Kind::Rectangle | Kind::Bilinear => 4,
            Kind::Polygon(n) => n,
            Kind::QuadTriangle => 6,
            Kind::Biquadratic => 9,
            Kind::Coons => 12,
            Kind::Bicubic | Kind::Tensor => 16,
        }
    }

    /// One dimensional patches are curves, drawn and never filled.
    pub fn is_curve(self) -> bool {
        matches!(self, Kind::Line | Kind::QuadSpline | Kind::CubicSpline | Kind::BezierSpline)
    }

    /// The points whose colors a shading interpolates.
    pub fn corners(self) -> &'static [usize] {
        match self {
            Kind::Line | Kind::QuadSpline | Kind::CubicSpline | Kind::BezierSpline => &[0, 1],
            Kind::Triangle | Kind::QuadTriangle => &[0, 1, 2],
            Kind::Rectangle | Kind::Bilinear | Kind::Biquadratic => &[0, 1, 2, 3],
            Kind::Bicubic => &[0, 3, 15, 12],
            // p00, p30, p33, p03
            Kind::Coons | Kind::Tensor => &[0, 9, 6, 3],
            Kind::Polygon(_) => &[0],
        }
    }
}

/// A piece of an outline: a line or a cubic Bézier curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Edge {
    Line(P3, P3),
    Curve(P3, P3, P3, P3),
}

/// One patch: its points and their point metas (nan when there is none).
#[derive(Debug, Clone, PartialEq)]
pub struct Patch {
    pub kind: Kind,
    pub pts: Vec<P3>,
    pub metas: Vec<f64>,
}

fn comb(terms: &[(f64, P3)]) -> P3 {
    let mut r = [0.0; 3];
    for (w, p) in terms {
        for i in 0..3 {
            r[i] += w * p[i];
        }
    }
    r
}

fn lerp(a: P3, b: P3, t: f64) -> P3 {
    comb(&[(1.0 - t, a), (t, b)])
}

/// Lagrange weights on 0, 1/2, 1.
fn quad_weights(t: f64) -> [f64; 3] {
    [2.0 * (t - 0.5) * (t - 1.0), 4.0 * t * (1.0 - t), 2.0 * t * (t - 0.5)]
}

/// Lagrange weights on 0, 1/3, 2/3, 1.
fn cubic_weights(t: f64) -> [f64; 4] {
    let (a, b, c) = (1.0 / 3.0, 2.0 / 3.0, 1.0);
    [
        (t - a) * (t - b) * (t - c) / ((0.0 - a) * (0.0 - b) * (0.0 - c)),
        t * (t - b) * (t - c) / (a * (a - b) * (a - c)),
        t * (t - a) * (t - c) / (b * (b - a) * (b - c)),
        t * (t - a) * (t - b) / (c * (c - a) * (c - b)),
    ]
}

/// Bernstein weights of degree 3.
fn bernstein(t: f64) -> [f64; 4] {
    let s = 1.0 - t;
    [s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t]
}

/// The biquadratic points on the 3 × 3 grid (u across, v up): A E B / H I F / D G C.
const BIQUAD_GRID: [[usize; 3]; 3] = [[0, 4, 1], [7, 8, 5], [3, 6, 2]];

/// The coons/tensor points as a 4 × 4 Bézier net p[i][j] (i along u, j along v), as PDF lists
/// them: the boundary from p00 up to p03, along to p33, down to p30, back to p10, then p11, p12,
/// p22, p21.
const COONS_NET: [[usize; 4]; 4] = [[0, 1, 2, 3], [11, 12, 13, 4], [10, 15, 14, 5], [9, 8, 7, 6]];

/// The cubic through four points at 0, 1/3, 2/3, 1, as Bézier control points.
fn lagrange_cubic(a: P3, b: P3, c: P3, d: P3) -> (P3, P3) {
    (comb(&[(-5.0 / 6.0, a), (3.0, b), (-1.5, c), (1.0 / 3.0, d)]), comb(&[(1.0 / 3.0, a), (-1.5, b), (3.0, c), (-5.0 / 6.0, d)]))
}

/// The quadratic through `a`, `m` (its middle) and `b`, as a cubic Bézier curve.
fn quad_edge(a: P3, m: P3, b: P3) -> Edge {
    // the quadratic's control point is 2m - (a+b)/2; a cubic's lie 2/3 of the way to it
    let q = comb(&[(2.0, m), (-0.5, a), (-0.5, b)]);
    Edge::Curve(a, lerp(a, q, 2.0 / 3.0), lerp(b, q, 2.0 / 3.0), b)
}

impl Patch {
    /// A value of the patch's interpolation of point metas at (u, v), or a point; for
    /// triangles (u, v) are the barycentric weights of B and C.
    fn interpolate<T: Copy>(&self, u: f64, v: f64, vals: &[T], combine: impl Fn(&[(f64, T)]) -> T) -> T {
        match self.kind {
            Kind::Biquadratic => {
                let (wu, wv) = (quad_weights(u), quad_weights(v));
                let mut terms = Vec::with_capacity(9);
                for (j, row) in BIQUAD_GRID.iter().enumerate() {
                    for (i, &k) in row.iter().enumerate() {
                        terms.push((wu[i] * wv[j], vals[k]));
                    }
                }
                combine(&terms)
            }
            Kind::Bicubic => {
                let (wu, wv) = (cubic_weights(u), cubic_weights(v));
                let mut terms = Vec::with_capacity(16);
                for j in 0..4 {
                    for i in 0..4 {
                        terms.push((wu[i] * wv[j], vals[j * 4 + i]));
                    }
                }
                combine(&terms)
            }
            Kind::QuadTriangle => {
                let (l1, l2, l3) = (1.0 - u - v, u, v);
                combine(&[
                    (l1 * (2.0 * l1 - 1.0), vals[0]),
                    (l2 * (2.0 * l2 - 1.0), vals[1]),
                    (l3 * (2.0 * l3 - 1.0), vals[2]),
                    (4.0 * l1 * l2, vals[3]),
                    (4.0 * l2 * l3, vals[4]),
                    (4.0 * l3 * l1, vals[5]),
                ])
            }
            Kind::Triangle => combine(&[(1.0 - u - v, vals[0]), (u, vals[1]), (v, vals[2])]),
            Kind::QuadSpline => {
                let w = quad_weights(u);
                combine(&[(w[0], vals[0]), (w[1], vals[2]), (w[2], vals[1])])
            }
            Kind::CubicSpline => {
                let w = cubic_weights(u);
                combine(&[(w[0], vals[0]), (w[1], vals[2]), (w[2], vals[3]), (w[3], vals[1])])
            }
            Kind::BezierSpline => {
                let w = bernstein(u);
                combine(&[(w[0], vals[0]), (w[1], vals[2]), (w[2], vals[3]), (w[3], vals[1])])
            }
            Kind::Line => combine(&[(1.0 - u, vals[0]), (u, vals[1])]),
            Kind::Coons | Kind::Tensor => {
                // a Bézier patch: the tensor net, its inner points those of the Coons patch
                // when not given
                let (wu, wv) = (bernstein(u), bernstein(v));
                let mut terms = Vec::with_capacity(16);
                for i in 0..4 {
                    for j in 0..4 {
                        terms.push((wu[i] * wv[j], vals[COONS_NET[i][j].min(vals.len() - 1)]));
                    }
                }
                combine(&terms)
            }
            // rectangles, bilinear patches and polygons: bilinear in the first four corners
            Kind::Rectangle | Kind::Bilinear | Kind::Polygon(_) => {
                let n = vals.len();
                combine(&[((1.0 - u) * (1.0 - v), vals[0]), (u * (1.0 - v), vals[1 % n]), (u * v, vals[2 % n]), ((1.0 - u) * v, vals[3 % n])])
            }
        }
    }

    /// The point at (u, v).
    pub fn eval(&self, u: f64, v: f64) -> P3 {
        if matches!(self.kind, Kind::Coons) {
            return self.coons(u, v);
        }
        self.interpolate(u, v, &self.pts, comb)
    }

    /// A Coons patch: the boundary curves blended, less the bilinear corners.
    fn coons(&self, u: f64, v: f64) -> P3 {
        let p = |i: usize, j: usize| self.pts[COONS_NET[i][j]];
        let curve = |a: P3, b: P3, c: P3, d: P3, t: f64| {
            let w = bernstein(t);
            comb(&[(w[0], a), (w[1], b), (w[2], c), (w[3], d)])
        };
        let c1 = curve(p(0, 0), p(1, 0), p(2, 0), p(3, 0), u);
        let c2 = curve(p(0, 3), p(1, 3), p(2, 3), p(3, 3), u);
        let d1 = curve(p(0, 0), p(0, 1), p(0, 2), p(0, 3), v);
        let d2 = curve(p(3, 0), p(3, 1), p(3, 2), p(3, 3), v);
        comb(&[(1.0 - v, c1), (v, c2), (1.0 - u, d1), (u, d2), (-(1.0 - u) * (1.0 - v), p(0, 0)), (-u * (1.0 - v), p(3, 0)), (-(1.0 - u) * v, p(0, 3)), (-u * v, p(3, 3))])
    }

    /// The point meta at (u, v), from the patch's points.
    fn meta_at(&self, u: f64, v: f64) -> f64 {
        if matches!(self.kind, Kind::Coons | Kind::Tensor) {
            return self.color_meta(u, v);
        }
        self.interpolate(u, v, &self.metas, |terms| terms.iter().map(|(w, m)| w * m).sum())
    }

    /// The meta of a patch of one color: the mean of its corners' (`foreach cdata vertex`) for
    /// the patches of higher order, of all its points otherwise; with `flat corner`, the first.
    pub fn flat_meta(&self, corner: bool) -> f64 {
        if corner {
            return self.metas.first().copied().unwrap_or(f64::NAN);
        }
        let all: Vec<usize> = (0..self.metas.len()).collect();
        let idx: &[usize] = match self.kind {
            Kind::QuadTriangle | Kind::Biquadratic | Kind::Bicubic | Kind::Coons | Kind::Tensor => self.kind.corners(),
            _ => &all,
        };
        idx.iter().map(|&k| self.metas[k]).sum::<f64>() / idx.len().max(1) as f64
    }

    /// The meta a shading has at (u, v): bilinear between the corners (barycentric for
    /// triangles).
    pub fn color_meta(&self, u: f64, v: f64) -> f64 {
        let c: Vec<f64> = self.kind.corners().iter().map(|&k| self.metas.get(k).copied().unwrap_or(f64::NAN)).collect();
        match c.len() {
            1 => c[0],
            2 => c[0] * (1.0 - u) + c[1] * u,
            3 => c[0] * (1.0 - u - v) + c[1] * u + c[2] * v,
            _ => (1.0 - u) * (1.0 - v) * c[0] + u * (1.0 - v) * c[1] + u * v * c[2] + (1.0 - u) * v * c[3],
        }
    }

    /// The patch's points and metas at the given parameters, as a patch of `kind`.
    fn sub(&self, kind: Kind, params: &[(f64, f64)]) -> Patch {
        Patch { kind, pts: params.iter().map(|&(u, v)| self.eval(u, v)).collect(), metas: params.iter().map(|&(u, v)| self.meta_at(u, v)).collect() }
    }

    /// `patch refines`: the patch cut in pieces of its own type, as pgfplots cuts it.
    pub fn refine(&self) -> Vec<Patch> {
        match self.kind {
            Kind::Biquadratic | Kind::Rectangle | Kind::Bilinear => {
                let grid = |u0: f64, v0: f64| -> Vec<(f64, f64)> {
                    let (u1, v1, um, vm) = (u0 + 0.5, v0 + 0.5, u0 + 0.25, v0 + 0.25);
                    if self.kind == Kind::Biquadratic {
                        vec![(u0, v0), (u1, v0), (u1, v1), (u0, v1), (um, v0), (u1, vm), (um, v1), (u0, vm), (um, vm)]
                    } else {
                        vec![(u0, v0), (u1, v0), (u1, v1), (u0, v1)]
                    }
                };
                [(0.0, 0.0), (0.5, 0.0), (0.5, 0.5), (0.0, 0.5)].iter().map(|&(u, v)| self.sub(self.kind, &grid(u, v))).collect()
            }
            Kind::QuadTriangle => {
                // corners as (u, v) = weights of B and C
                let (a, b, c) = ((0.0, 0.0), (1.0, 0.0), (0.0, 1.0));
                let mid = |p: (f64, f64), q: (f64, f64)| ((p.0 + q.0) / 2.0, (p.1 + q.1) / 2.0);
                let (d, e, f) = (mid(a, b), mid(b, c), mid(c, a));
                [(f, a, d), (f, d, e), (d, b, e), (e, c, f)].iter().map(|&(p, q, r)| self.sub(Kind::QuadTriangle, &[p, q, r, mid(p, q), mid(q, r), mid(r, p)])).collect()
            }
            Kind::QuadSpline => [(0.0, 0.5), (0.5, 1.0)].iter().map(|&(s, t)| self.sub(Kind::QuadSpline, &[(s, 0.0), (t, 0.0), ((s + t) / 2.0, 0.0)])).collect(),
            Kind::CubicSpline => [0.0, 1.0 / 3.0, 2.0 / 3.0]
                .iter()
                .map(|&s| {
                    let h = 1.0 / 3.0;
                    self.sub(Kind::CubicSpline, &[(s, 0.0), (s + h, 0.0), (s + h / 3.0, 0.0), (s + 2.0 * h / 3.0, 0.0)])
                })
                .collect(),
            Kind::Line => [(0.0, 0.5), (0.5, 1.0)].iter().map(|&(s, t)| self.sub(Kind::Line, &[(s, 0.0), (t, 0.0)])).collect(),
            Kind::Triangle => {
                // longest edge bisection
                let d = |i: usize, j: usize| {
                    let (p, q) = (self.pts[i], self.pts[j]);
                    (0..3).map(|k| (p[k] - q[k]).powi(2)).sum::<f64>()
                };
                let (i, j, k) = [(0, 1, 2), (1, 2, 0), (2, 0, 1)].into_iter().fold((0, 1, 2), |best, e| if d(e.0, e.1) > d(best.0, best.1) { e } else { best });
                let m = lerp(self.pts[i], self.pts[j], 0.5);
                let mm = (self.metas[i] + self.metas[j]) / 2.0;
                vec![
                    Patch { kind: Kind::Triangle, pts: vec![self.pts[i], m, self.pts[k]], metas: vec![self.metas[i], mm, self.metas[k]] },
                    Patch { kind: Kind::Triangle, pts: vec![m, self.pts[j], self.pts[k]], metas: vec![mm, self.metas[j], self.metas[k]] },
                ]
            }
            _ => vec![self.clone()],
        }
    }

    /// `patch to triangles`: flat triangles through the patch's points.
    pub fn triangulate(&self) -> Vec<Patch> {
        let tris: &[[usize; 3]] = match self.kind {
            Kind::Rectangle | Kind::Bilinear => &[[0, 1, 3], [1, 2, 3]],
            Kind::Biquadratic => &[[0, 4, 8], [7, 0, 8], [4, 1, 8], [1, 5, 8], [3, 7, 8], [3, 6, 8], [6, 2, 8], [2, 5, 8]],
            Kind::QuadTriangle => &[[5, 0, 3], [5, 3, 4], [3, 1, 4], [4, 2, 5]],
            _ => return vec![self.clone()],
        };
        tris.iter().map(|t| Patch { kind: Kind::Triangle, pts: t.iter().map(|&k| self.pts[k]).collect(), metas: t.iter().map(|&k| self.metas[k]).collect() }).collect()
    }

    /// The outline, as lines and cubic Bézier curves.
    pub fn outline(&self) -> Vec<Edge> {
        let p = &self.pts;
        let poly = |idx: &[usize]| (0..idx.len()).map(|i| Edge::Line(p[idx[i]], p[idx[(i + 1) % idx.len()]])).collect::<Vec<_>>();
        match self.kind {
            Kind::Line => vec![Edge::Line(p[0], p[1])],
            Kind::QuadSpline => vec![quad_edge(p[0], p[2], p[1])],
            Kind::CubicSpline => {
                let (c1, c2) = lagrange_cubic(p[0], p[2], p[3], p[1]);
                vec![Edge::Curve(p[0], c1, c2, p[1])]
            }
            Kind::BezierSpline => vec![Edge::Curve(p[0], p[2], p[3], p[1])],
            Kind::Triangle => poly(&[0, 1, 2]),
            Kind::Rectangle | Kind::Bilinear => poly(&[0, 1, 2, 3]),
            Kind::Polygon(n) => poly(&(0..n.min(p.len())).collect::<Vec<_>>()),
            Kind::QuadTriangle => vec![quad_edge(p[0], p[3], p[1]), quad_edge(p[1], p[4], p[2]), quad_edge(p[2], p[5], p[0])],
            Kind::Biquadratic => vec![quad_edge(p[0], p[4], p[1]), quad_edge(p[1], p[5], p[2]), quad_edge(p[2], p[6], p[3]), quad_edge(p[3], p[7], p[0])],
            Kind::Bicubic => [[0, 1, 2, 3], [3, 7, 11, 15], [15, 14, 13, 12], [12, 8, 4, 0]]
                .iter()
                .map(|e| {
                    let (c1, c2) = lagrange_cubic(p[e[0]], p[e[1]], p[e[2]], p[e[3]]);
                    Edge::Curve(p[e[0]], c1, c2, p[e[3]])
                })
                .collect(),
            Kind::Coons | Kind::Tensor => (0..4).map(|k| Edge::Curve(p[3 * k], p[3 * k + 1], p[3 * k + 2], p[(3 * k + 3) % 12])).collect(),
        }
    }

    /// The patch cut into `n` × `n` small pieces (n² triangles for a triangle), each its points
    /// and the shading's meta at its center: the parameters run along u, then v.
    /// The patch as `n` by `n` cells (`n` to a side of a triangle), each as two triangles with the
    /// point meta at their corners: shaded linearly across each, the patch's colors change
    /// smoothly, in no bands.
    pub fn shaded_triangles(&self, n: usize) -> Vec<[(P3, f64); 3]> {
        let nf = n as f64;
        let at = |u: f64, v: f64| (self.eval(u, v), self.color_meta(u, v));
        let mut out = Vec::new();
        match self.kind {
            Kind::Triangle | Kind::QuadTriangle => {
                let p = |i: usize, j: usize| at(i as f64 / nf, j as f64 / nf);
                for j in 0..n {
                    for i in 0..n - j {
                        out.push([p(i, j), p(i + 1, j), p(i, j + 1)]);
                        if i + j + 1 < n {
                            out.push([p(i + 1, j), p(i + 1, j + 1), p(i, j + 1)]);
                        }
                    }
                }
            }
            _ => {
                for j in 0..n {
                    for i in 0..n {
                        let (u0, u1, v0, v1) = (i as f64 / nf, (i + 1) as f64 / nf, j as f64 / nf, (j + 1) as f64 / nf);
                        let q = [at(u0, v0), at(u1, v0), at(u1, v1), at(u0, v1)];
                        out.push([q[0], q[1], q[2]]);
                        out.push([q[0], q[2], q[3]]);
                    }
                }
            }
        }
        out
    }

    /// The outline of [`Self::shaded_triangles`]`(n)`: the patch's sides cut as its triangles cut them.
    pub fn shaded_outline(&self, n: usize) -> Vec<P3> {
        let t = |k: usize| k as f64 / n as f64;
        match self.kind {
            Kind::Triangle | Kind::QuadTriangle => {
                let mut v: Vec<P3> = (0..n).map(|k| self.eval(t(k), 0.0)).collect();
                v.extend((0..n).map(|k| self.eval(1.0 - t(k), t(k))));
                v.extend((0..n).map(|k| self.eval(0.0, 1.0 - t(k))));
                v
            }
            _ => {
                let mut v: Vec<P3> = (0..n).map(|k| self.eval(t(k), 0.0)).collect();
                v.extend((0..n).map(|k| self.eval(1.0, t(k))));
                v.extend((0..n).map(|k| self.eval(1.0 - t(k), 1.0)));
                v.extend((0..n).map(|k| self.eval(0.0, 1.0 - t(k))));
                v
            }
        }
    }

    pub fn pieces(&self, n: usize) -> Vec<(Vec<P3>, f64)> {
        let nf = n as f64;
        let mut out = Vec::new();
        match self.kind {
            Kind::Triangle | Kind::QuadTriangle => {
                let at = |i: usize, j: usize| (i as f64 / nf, j as f64 / nf);
                let mut tri = |a: (f64, f64), b: (f64, f64), c: (f64, f64)| {
                    let (cu, cv) = ((a.0 + b.0 + c.0) / 3.0, (a.1 + b.1 + c.1) / 3.0);
                    out.push((vec![self.eval(a.0, a.1), self.eval(b.0, b.1), self.eval(c.0, c.1)], self.color_meta(cu, cv)));
                };
                for j in 0..n {
                    for i in 0..n - j {
                        tri(at(i, j), at(i + 1, j), at(i, j + 1));
                        if i + j + 1 < n {
                            tri(at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
                        }
                    }
                }
            }
            _ => {
                for j in 0..n {
                    for i in 0..n {
                        let (u0, u1, v0, v1) = (i as f64 / nf, (i + 1) as f64 / nf, j as f64 / nf, (j + 1) as f64 / nf);
                        let pts = vec![self.eval(u0, v0), self.eval(u1, v0), self.eval(u1, v1), self.eval(u0, v1)];
                        out.push((pts, self.color_meta((u0 + u1) / 2.0, (v0 + v1) / 2.0)));
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn biquad() -> Patch {
        let pts = vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 2.0, 0.0], [0.0, 2.0, 0.0], [1.0, -0.5, 0.0], [2.0, 1.0, 0.0], [1.0, 2.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 1.0]];
        Patch { kind: Kind::Biquadratic, pts, metas: vec![1.0, 2.0, 3.0, 4.0, 0.0, 0.0, 0.0, 0.0, 9.0] }
    }

    #[test]
    fn biquadratic_interpolates_its_points() {
        let p = biquad();
        assert_eq!(p.eval(0.5, 0.0), [1.0, -0.5, 0.0]);
        assert_eq!(p.eval(0.5, 0.5), [1.0, 1.0, 1.0]);
        assert_eq!(p.eval(1.0, 1.0), [2.0, 2.0, 0.0]);
        // the shading's color comes from the corners only
        assert_eq!(p.color_meta(0.5, 0.5), 2.5);
        let parts = p.refine();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0].pts[2], [1.0, 1.0, 1.0]);
        assert_eq!(parts[0].metas[2], 9.0);
    }

    #[test]
    fn triangle_quadr_and_splines() {
        let t = Patch {
            kind: Kind::QuadTriangle,
            pts: vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 0.0], [1.0, -1.0, 0.0], [1.0, 1.0, 0.0], [-1.0, 1.0, 0.0]],
            metas: vec![0.0; 6],
        };
        assert_eq!(t.eval(0.5, 0.0), [1.0, -1.0, 0.0]);
        assert_eq!(t.refine().len(), 4);
        assert_eq!(t.triangulate().len(), 4);
        let s = Patch { kind: Kind::QuadSpline, pts: vec![[0.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.5, 0.25, 0.0]], metas: vec![0.0; 3] };
        let Edge::Curve(_, c1, c2, _) = s.outline()[0] else { panic!() };
        // the parabola y = x²: control points at (1/3, 0) and (2/3, 1/3)
        assert!((c1[0] - 1.0 / 3.0).abs() < 1e-12 && c1[1].abs() < 1e-12);
        assert!((c2[1] - 1.0 / 3.0).abs() < 1e-12);
    }

    // What no figure reaches yet (a Bézier spline is never refined, a triangle is shaded exactly,
    // a Coons patch is never refined) still answers as its patch type says.
    #[test]
    fn bezier_splines_triangles_and_coons_patches_cut_and_colored() {
        // a Bézier spline: its point is the Bézier curve's, its color goes from one end to the other
        let b = Patch { kind: Kind::BezierSpline, pts: vec![[0.0, 0.0, 0.0], [3.0, 0.0, 0.0], [1.0, 1.0, 0.0], [2.0, 1.0, 0.0]], metas: vec![0.0, 1.0, 0.0, 0.0] };
        assert_eq!(b.eval(0.5, 0.0), [1.5, 0.75, 0.0]);
        assert_eq!(b.color_meta(0.25, 0.0), 0.25);
        // a triangle in n² small triangles, its outline cut where they cut it
        let t = Patch { kind: Kind::Triangle, pts: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], metas: vec![0.0, 1.0, 2.0] };
        let tris = t.shaded_triangles(2);
        assert_eq!(tris.len(), 4);
        assert_eq!(tris[0][1], ([0.5, 0.0, 0.0], 0.5));
        assert_eq!(t.shaded_outline(2), vec![[0.0, 0.0, 0.0], [0.5, 0.0, 0.0], [1.0, 0.0, 0.0], [0.5, 0.5, 0.0], [0.0, 1.0, 0.0], [0.0, 0.5, 0.0]]);
        // a Coons patch: its meta bilinear between its corners
        let mut metas = vec![0.0; 12];
        (metas[0], metas[9], metas[6], metas[3]) = (0.0, 1.0, 2.0, 3.0);
        let c = Patch { kind: Kind::Coons, pts: vec![[0.0; 3]; 12], metas };
        assert_eq!(c.meta_at(0.5, 0.5), 1.5);
    }
}
