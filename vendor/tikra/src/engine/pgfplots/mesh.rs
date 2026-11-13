//! `mesh`, `surf` and `patch` plots (`pgfplotsmeshplothandler.code.tex`): a lattice or a list of
//! patches, each colored by the colormap at its point meta, drawn back to front in 3d
//! (`z buffer`), with the shaders `faceted` (fill and `faceted color` edges), `flat` (one color,
//! edges too), `flat corner` and `interp` (the point meta shaded across each triangle).
//!
//! Colors given with `mapped color` (`draw=mapped color!80!black`) are linear in it: each is found
//! by setting the style with `mapped color` black, then white.

use crate::canvas::{Fill, Item, Paint, Stroke};
use crate::color::Color;
use crate::scene::{Point, Segment};
use crate::tex::Token;
use crate::Error;

use super::draw::Frame;
use super::keys;
use super::plot::Plot;
use super::Engine;

/// A color made from `mapped color`: `base + mapped × slope`, channel by channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Linear {
    pub base: Color,
    pub slope: Color,
}

impl Linear {
    pub fn from(black: Color, white: Color) -> Linear {
        Linear { base: black, slope: Color::rgb(white.r - black.r, white.g - black.g, white.b - black.b) }
    }
    pub fn at(&self, m: Color) -> Color {
        let c = |b: f64, s: f64, m: f64| (b + s * m).clamp(0.0, 1.0);
        Color::rgb(c(self.base.r, self.slope.r, m.r), c(self.base.g, self.slope.g, m.g), c(self.base.b, self.slope.b, m.b))
    }
}

/// The state's colors as functions of `mapped color`: (draw, fill) after `f` runs in a group.
pub fn mapped_colors(e: &mut Engine, mut f: impl FnMut(&mut Engine) -> Result<(), Error>) -> Result<(Linear, Linear, super::super::GState), Error> {
    let mut states = Vec::new();
    // (pgfplots's `mapped color` is an rgb color)
    for m in [Color::rgb(0.0, 0.0, 0.0), Color::rgb(1.0, 1.0, 1.0)] {
        e.colors.insert("mapped color".into(), m);
        e.begin_group();
        let r = f(e);
        let st = e.st.clone();
        e.end_group();
        r?;
        states.push(st);
    }
    // what is drawn later with `mapped color` (nodes near coords) gets TeX's black
    e.colors.insert("mapped color".into(), Color::rgb(0.0, 0.0, 0.0));
    let draw = Linear::from(states[0].stroke_color(), states[1].stroke_color());
    let fill = Linear::from(states[0].fill_paint_color(), states[1].fill_paint_color());
    Ok((draw, fill, states.swap_remove(1)))
}

/// A color as a function of `mapped color`.
fn mapped_named(e: &mut Engine, name: &str) -> Option<Linear> {
    e.colors.insert("mapped color".into(), Color::rgb(0.0, 0.0, 0.0));
    let a = e.color(name)?;
    e.colors.insert("mapped color".into(), Color::rgb(1.0, 1.0, 1.0));
    let b = e.color(name);
    e.colors.insert("mapped color".into(), Color::rgb(0.0, 0.0, 0.0));
    Some(Linear::from(a, b?))
}

/// A patch: its corners (canvas points) and the points its colors come from.
type PatchSource = (Vec<usize>, Vec<usize>);

/// The colormap's color for meta value `m`.
pub fn map(f: &Frame, m: f64) -> Color {
    match &f.colormap {
        Some(c) => c.color_of(m, f.meta[0], f.meta[1]),
        None => Color::BLACK,
    }
}

/// The limits `mesh input=image` asks for, as pgfplots' heuristic finds them while it reads the
/// points: half a step before the first point of each scanline and after its last (along the
/// scanlines, axis `a`), and half a step beyond the first point of each scanline but the first, on
/// both sides (across them). `pts` are the centers, scanline by scanline.
pub fn image_limits(pts: &[[f64; 3]], rows: usize, cols: usize, a: usize) -> Vec<[f64; 3]> {
    let b = 1 - a;
    let mut out = Vec::new();
    let point = |va: f64, vb: f64| {
        let mut p = [0.0; 3];
        p[a] = va;
        p[b] = vb;
        p
    };
    let mut last_b: Option<f64> = None;
    let mut b_next = false;
    for s in 0..rows {
        let (mut last_a, mut lastlast_a): (Option<f64>, Option<f64>) = (None, None);
        let mut a_next = true;
        let mut cur_b = f64::NAN;
        for k in 0..cols {
            let Some(c) = pts.get(s * cols + k) else { break };
            let (ca, cb) = (c[a], c[b]);
            cur_b = cb;
            if let (true, Some(la)) = (a_next, last_a) {
                out.push(point(la - 0.5 * (ca - la), cb));
                a_next = false;
            }
            if let (true, Some(lb)) = (b_next, last_b) {
                out.push(point(ca, lb - 0.5 * (cb - lb)));
                out.push(point(ca, cb + 0.5 * (cb - lb)));
                b_next = false;
            }
            lastlast_a = last_a;
            last_a = Some(ca);
            last_b = Some(cb);
        }
        if let (Some(la), Some(lla)) = (last_a, lastlast_a) {
            out.push(point(la + 0.5 * (la - lla), cur_b));
        }
        b_next = true;
    }
    out
}

/// `mesh input=image` with `shader=interp`: the point metas of each cell's corners, as
/// `pgfplotsmeshplotimage.code.tex` sweeps the scanlines. A cell's first corner has its own meta;
/// an inner corner the meta of the center after it if bounded, else the mean of the four centers
/// around; a corner of the first row or column the meta of its center after it, else an
/// extrapolation; those of the last row and column extrapolations (`2 × mean of two centers −
/// the corner before`), the very last one the last center's. An unbounded meta counts as the
/// lowest (`\pgfplots@metamin`).
pub fn image_interp_metas(metas: &[f64], rows: usize, cols: usize, min: f64) -> Vec<[f64; 4]> {
    // (a plot is a lattice of at least 2 × 2 points, one meta each)
    debug_assert!(rows >= 2 && cols >= 2 && metas.len() >= rows * cols, "an image of {rows} × {cols} cells");
    let raw = |r: usize, c: usize| metas[r * cols + c];
    let m = |r: usize, c: usize| Some(raw(r, c)).filter(|v| v.is_finite()).unwrap_or(min);
    let or = |r: usize, c: usize, v: f64| Some(raw(r, c)).filter(|v| v.is_finite()).unwrap_or(v);
    // k[at(a, b)]: the corner between center rows a-1, a and columns b-1, b
    let mut k = vec![0.0; (rows + 1) * (cols + 1)];
    let at = |a: usize, b: usize| a * (cols + 1) + b;
    for a in 1..rows {
        for b in 1..cols {
            k[at(a, b)] = or(a, b, 0.25 * (m(a - 1, b - 1) + m(a - 1, b) + m(a, b) + m(a, b - 1)));
        }
    }
    for b in 1..cols {
        k[at(0, b)] = or(0, b, m(0, b) + m(0, b - 1) - k[at(1, b)]);
    }
    for a in 1..rows {
        k[at(a, 0)] = or(a, 0, m(a, 0) + m(a - 1, 0) - k[at(a, 1)]);
    }
    for a in 1..rows {
        k[at(a, cols)] = m(a - 1, cols - 1) + m(a, cols - 1) - k[at(a, cols - 1)];
    }
    k[at(0, cols)] = k[at(1, cols)] + k[at(0, cols - 1)] - k[at(1, cols - 1)];
    for b in 1..cols {
        k[at(rows, b)] = m(rows - 1, b - 1) + m(rows - 1, b) - k[at(rows - 1, b)];
    }
    k[at(rows, 0)] = k[at(rows, 1)] + k[at(rows - 1, 0)] - k[at(rows - 1, 1)];
    k[at(rows, cols)] = or(rows - 1, cols - 1, k[at(rows, cols - 1)] - k[at(rows - 1, cols - 1)] + k[at(rows - 1, cols)]);
    let mut out = Vec::with_capacity(rows * cols);
    for r in 0..rows {
        for c in 0..cols {
            out.push([raw(r, c), k[at(r, c + 1)], k[at(r + 1, c + 1)], k[at(r + 1, c)]]);
        }
    }
    out
}

/// `mesh input=image`: the corners of the cells around a lattice of centers, (rows+1) × (cols+1),
/// halfway between neighbours and as far out at the borders.
pub fn image_corners(pts: &[[f64; 3]], rows: usize, cols: usize) -> Vec<[f64; 3]> {
    let c = |r: isize, k: isize| -> [f64; 3] {
        // a center, extrapolated linearly outside the lattice
        let (rr, kk) = (r.clamp(0, rows as isize - 1), k.clamp(0, cols as isize - 1));
        let get = |r: isize, k: isize| pts.get(r as usize * cols + k as usize).copied().unwrap_or([f64::NAN; 3]);
        let base = get(rr, kk);
        let mut v = base;
        if r != rr && rows > 1 {
            let other = get(if r < 0 { 1 } else { rows as isize - 2 }, kk);
            for i in 0..3 {
                v[i] += base[i] - other[i];
            }
        }
        if k != kk && cols > 1 {
            let other = get(rr, if k < 0 { 1 } else { cols as isize - 2 });
            for i in 0..3 {
                v[i] += base[i] - other[i];
            }
        }
        v
    };
    let mut out = Vec::with_capacity((rows + 1) * (cols + 1));
    for r in 0..=rows as isize {
        for k in 0..=cols as isize {
            let q = [c(r - 1, k - 1), c(r - 1, k), c(r, k - 1), c(r, k)];
            out.push([0, 1, 2].map(|i| q.iter().map(|p| p[i]).sum::<f64>() / 4.0));
        }
    }
    out
}

/// The patches of a plot: corner indices.
fn patches(p: &Plot, n: usize, kind: &str, vertex_count: usize) -> Vec<Vec<usize>> {
    if let Some((rows, cols)) = p.lattice {
        let mut v = Vec::new();
        // `patch type=line`: the segments along the scanlines only
        if kind == "line" {
            for r in 0..rows {
                for c in 0..cols.saturating_sub(1) {
                    v.push(vec![r * cols + c, r * cols + c + 1]);
                }
            }
            return v;
        }
        for r in 0..rows.saturating_sub(1) {
            for c in 0..cols.saturating_sub(1) {
                let k = r * cols + c;
                v.push(vec![k, k + 1, k + cols + 1, k + cols]);
            }
        }
        return v;
    }
    if p.patch {
        // each patch's outline: its corners in turn (a bicubic's are 0, 3, 15, 12), a polygon's
        // `vertex count` points
        let kind = super::patch::Kind::from_name(kind, vertex_count);
        let size = kind.size();
        let corners: Vec<usize> = match kind {
            super::patch::Kind::Polygon(n) => (0..n).collect(),
            k => k.corners().to_vec(),
        };
        return (0..n / size).map(|i| corners.iter().map(|k| i * size + k).collect()).collect();
    }
    // a line of points: segments
    (0..n.saturating_sub(1)).map(|i| vec![i, i + 1]).collect()
}

/// Draws a mesh, surf or patch plot (in the plot's group, its style set).
pub fn draw(e: &mut Engine, p: &Plot, pts: &[[f64; 3]], f: &Frame, style: &[Token]) -> Result<(), Error> {
    let (draw, fill, st) = mapped_colors(e, |e| {
        e.st.mode = Default::default();
        e.process_keys(style.to_vec(), Some("/pgfplots"))
    })?;
    let surf = p.mesh == Some('s');
    let shader = keys::value(e, "/pgfplots/shader");
    let faceted_name = keys::value(e, "/pgfplots/faceted color");
    let faceted = mapped_named(e, &faceted_name).unwrap_or(draw);
    let kind = keys::value(e, "/pgfplots/patch type");
    let zbuffer = keys::value(e, "/pgfplots/z buffer");
    let width = st.line_width;
    let opacity = (st.draw_opacity, st.fill_opacity);
    let explicit = keys::value(e, "/pgfplots/mesh/color input").starts_with("explicit");
    let vertex_count = keys::number(e, "/pgfplots/vertex count").unwrap_or(4.0).max(1.0) as usize;
    if p.patch && p.lattice.is_none() && !explicit {
        let look = Look { draw, fill, faceted, shader, stroke: st.clone(), surf };
        return draw_patches(e, p, pts, f, &kind, &look);
    }
    let to_canvas = |l: &[f64; 3]| if l.iter().take(if f.three_d { 3 } else { 2 }).all(|v| v.is_finite()) { Some(f.at(*l)) } else { None };
    // each patch: its corners (into `canvas`) and the points its colors come from
    let (canvas, mut list): (Vec<Option<Point>>, Vec<PatchSource>) = match p.lattice.filter(|_| p.image) {
        Some((rows, cols)) => {
            let corners = image_corners(pts, rows, cols);
            let mut v = Vec::new();
            for r in 0..rows {
                for c in 0..cols {
                    let k = r * (cols + 1) + c;
                    v.push((vec![k, k + 1, k + cols + 2, k + cols + 1], vec![r * cols + c; 4]));
                }
            }
            (corners.iter().map(to_canvas).collect(), v)
        }
        None => (pts.iter().map(to_canvas).collect(), patches(p, pts.len(), &kind, vertex_count).into_iter().map(|q| (q.clone(), q)).collect()),
    };
    // `z buffer=auto` reverses the columns when the face at xmin is in front, the rows when the
    // one at ymin is: the lattice cells' corners turn with them
    let reversed = {
        let az = keys::number(e, "/pgfplots/view/az").unwrap_or(25.0).to_radians();
        let el = keys::number(e, "/pgfplots/view/el").unwrap_or(30.0).to_radians();
        if f.three_d {
            (az.sin() * el.cos() < -1e-9, -az.cos() * el.cos() < -1e-9)
        } else {
            (false, false)
        }
    };
    match (p.lattice.filter(|_| !p.image), zbuffer.as_str()) {
        _ if !f.three_d || zbuffer == "none" => {}
        // a matrix is drawn in its order, the rows or columns reversed (`z buffer=auto`)
        (Some((_, cols)), "default" | "auto") if cols > 1 => {
            let key = |q: &[usize]| {
                let (r, c) = (q[0] / cols, q[0] % cols);
                (if reversed.1 { usize::MAX - r } else { r }, if reversed.0 { usize::MAX - c } else { c })
            };
            list.sort_by_key(|(q, _)| key(q));
        }
        _ => {
            let depth = depth_fn(e, f);
            let of = |q: &[usize]| depth(&q.iter().map(|&k| pts[k]).collect::<Vec<_>>());
            list.sort_by(|(a, _), (b, _)| of(a).total_cmp(&of(b)));
        }
    }
    let closes = list.first().is_some_and(|(q, _)| q.len() > 2);
    // the plot's colormap (`colormap/blackwhite` among its options), and that of the patches
    // seen from behind (`mesh/interior colormap`)
    let parse_map = super::draw::colormap_named;
    let name = keys::value(e, "/pgfplots/colormap name");
    let outer = parse_map(e, &name).or_else(|| f.colormap.clone());
    let inner_name = keys::value(e, "/pgfplots/mesh/interior colormap name");
    let inner = if inner_name.is_empty() || !f.three_d { None } else { parse_map(e, &inner_name) };
    let thresh = keys::number(e, "/pgfplots/mesh/interior colormap thresh").unwrap_or(0.0);
    let is_inside = interior_fn(e, f, thresh);
    // `point meta rel=per plot`: the colormap spans this plot's point meta only
    let range = if keys::value(e, "/pgfplots/point meta rel") == "per plot" { plot_meta_range(p).unwrap_or(f.meta) } else { f.meta };
    // the shadings of `interp` are drawn as PDF viewers draw them, without antialiasing: no seams
    // between patches
    let crisp = closes && (surf || p.patch) && (shader == "interp" || shader == "faceted interp");
    if crisp {
        e.canvas.push(Item::Crisp);
    }
    // an image's cells are shaded between metas found for their corners
    let image_metas =
        p.lattice.filter(|_| p.image && crisp && !explicit).map(|(rows, cols)| image_interp_metas(&p.coords.iter().map(|c| c.meta).collect::<Vec<_>>(), rows, cols, range[0]));
    for (q, src) in list {
        let Some(corners) = q.iter().map(|&k| canvas.get(k).copied().flatten()).collect::<Option<Vec<Point>>>() else { continue };
        // the normal at the corner the z buffer made first (`compute normal`), its sign kept
        let inside = || {
            let c: Vec<[f64; 3]> = q.iter().map(|&k| pts[k]).collect();
            match (c.len(), reversed) {
                (4, (false, true)) => is_inside(c[3], c[2], c[0], -1.0),
                (4, (true, false)) => is_inside(c[1], c[0], c[2], -1.0),
                (4, (true, true)) => is_inside(c[2], c[3], c[1], 1.0),
                _ => is_inside(c[0], c[1], c[c.len() - 1], 1.0),
            }
        };
        let cmap = match &inner {
            Some(c) if q.len() > 2 && inside() => Some(c),
            _ => outer.as_ref(),
        };
        let map = |_: &Frame, m: f64| cmap.map_or(Color::BLACK, |c| c.color_of(m, range[0], range[1]));
        let metas: Vec<f64> = src.iter().map(|&k| p.coords.get(k).map_or(f64::NAN, |c| c.meta)).filter(|m| m.is_finite()).collect();
        // a patch whose color comes from a point meta of nan is not drawn (with `flat corner`, the
        // first corner's)
        let meta_of = |k: usize| p.coords.get(k).map_or(f64::NAN, |c| c.meta);
        let unbounded = if shader == "flat corner" { src.first().is_some_and(|&k| !meta_of(k).is_finite()) } else { metas.len() < src.len() };
        if p.has_meta && !explicit && unbounded {
            continue;
        }
        let metas = match &image_metas {
            Some(im) => im.get(src[0]).map_or(metas, |c| c.to_vec()),
            None => metas,
        };
        let m = if shader == "flat corner" { metas.first().copied().unwrap_or(f64::NAN) } else { metas.iter().sum::<f64>() / metas.len().max(1) as f64 };
        // `mesh/color input=explicit`: each point has its color
        let vertex: Option<Vec<Color>> = match explicit.then(|| src.iter().map(|&k| vertex_color(e, p, k)).collect::<Option<Vec<Color>>>()) {
            // a point without a color (`[]`) is unbounded, as a nan
            Some(None) => continue,
            v => v.flatten(),
        };
        let mapped = match &vertex {
            Some(v) if shader == "flat corner" => v[0],
            Some(v) => mean(v),
            None if m.is_finite() => map(f, m),
            None => Color::BLACK,
        };
        // interp: a large patch is shaded across, drawn as small pieces interpolated between its
        // corners
        let interp_ok = vertex.is_some() || metas.len() == corners.len();
        // a parallelogram whose color changes along one side only is an axial shading (as the
        // colorbar's): exact, without pieces
        if closes && shader == "interp" && vertex.is_none() && corners.len() == 4 && metas.len() == 4 {
            if let Some(cm) = cmap.filter(|c| c.access == super::colormap::Access::Map) {
                let same = |a: f64, b: f64| (a - b).abs() <= 1e-12 * (a.abs() + b.abs() + 1.0);
                let pick = if same(metas[0], metas[1]) && same(metas[3], metas[2]) {
                    Some((corners[0], corners[1], corners[3], corners[2], metas[0], metas[3]))
                } else if same(metas[0], metas[3]) && same(metas[1], metas[2]) {
                    Some((corners[0], corners[3], corners[1], corners[2], metas[0], metas[1]))
                } else {
                    None
                };
                if let Some((a, b, d, c, ma, mb)) = pick {
                    let flat = c.minus(b.plus(d).minus(a)).norm() < 1e-6 * (b.minus(a).norm() + d.minus(a).norm() + 1e-12);
                    if flat && !same(ma, mb) {
                        let (u, v) = (b.minus(a), d.minus(a));
                        let transform =
                            crate::scene::Transform { a: u.x / 100.0, b: u.y / 100.0, c: v.x / 100.0, d: v.y / 100.0, e: a.x + 0.5 * (u.x + v.x), f: a.y + 0.5 * (u.y + v.y) };
                        let (lo_m, hi_m) = (f.meta[0], f.meta[1]);
                        let mut metas_at: Vec<f64> = vec![ma, mb];
                        for (p, _) in cm.normalized() {
                            let m = lo_m + p * (hi_m - lo_m);
                            if (m - ma) * (m - mb) < 0.0 {
                                metas_at.push(m);
                            }
                        }
                        let mut stops: Vec<crate::canvas::Stop> = metas_at.iter().map(|m| ((m - ma) / (mb - ma), fill.at(map(f, *m)))).collect();
                        stops.sort_by(|x, y| x.0.total_cmp(&y.0));
                        let segs = vec![Segment::MoveTo(a), Segment::LineTo(b), Segment::LineTo(c), Segment::LineTo(d), Segment::Close];
                        if !e.canvas_frozen {
                            e.canvas.include_path(&segs);
                        }
                        let paint = Paint::Shading(crate::canvas::Shading::Axial { transform, stops });
                        e.canvas.push(Item::Path { segments: segs, fill: Some(Fill { paint, opacity: opacity.1, even_odd: false }), stroke: None });
                        continue;
                    }
                }
            }
        }
        // `colormap access=piecewise constant`: the shading's colors are bands, cut exactly
        if let Some(cm) = cmap.filter(|c| c.access == super::colormap::Access::Const && closes && shader == "interp" && vertex.is_none() && metas.len() == corners.len()) {
            // (a cell of a matrix: only patches of explicit colors have three corners here)
            let tris: Vec<[usize; 3]> = match reversed {
                (false, false) | (true, true) => vec![[0, 1, 3], [1, 2, 3]],
                _ => vec![[0, 1, 2], [0, 2, 3]],
            };
            let n = cm.stops.len();
            let width = (range[1] - range[0]) / n as f64;
            for t in tris {
                let tri: Vec<(Point, f64)> = t.iter().map(|&k| (corners[k], metas[k])).collect();
                for (b, (_, c)) in cm.stops.iter().enumerate() {
                    let lo = if b == 0 { f64::NEG_INFINITY } else { range[0] + b as f64 * width };
                    let hi = if b + 1 == n { f64::INFINITY } else { range[0] + (b + 1) as f64 * width };
                    let poly = super::contour::clip(&tri, lo, hi);
                    if poly.len() < 3 {
                        continue;
                    }
                    let c = fill.at(*c);
                    let mut segs = vec![Segment::MoveTo(poly[0])];
                    segs.extend(poly[1..].iter().map(|q| Segment::LineTo(*q)));
                    segs.push(Segment::Close);
                    if !e.canvas_frozen {
                        e.canvas.include_path(&segs);
                    }
                    let stroke = Stroke { color: c, width: 0.3 * crate::PT, dash: None, cap: st.cap, join: st.join, miter_limit: st.miter_limit, opacity: opacity.0 };
                    e.canvas.push(Item::Path { segments: segs, fill: Some(Fill { paint: Paint::Solid(c), opacity: opacity.1, even_odd: false }), stroke: Some(stroke) });
                }
            }
            continue;
        }
        // a triangle, or a cell of a matrix shaded as two triangles (a shading of type 5), ABD
        // then BCD of its corners as the z buffer has turned them (reversing the rows or the
        // columns): each a linear gradient of the colormap, however small
        let lattice_cell = corners.len() == 4 && p.lattice.is_some() && !p.image && kind != "bilinear";
        let shaded = shader == "interp" || shader == "faceted interp";
        if let Some(cm) = cmap.filter(|c| c.access == super::colormap::Access::Map && closes && shaded && vertex.is_none() && metas.len() == corners.len()) {
            // (four corners: only patches of explicit colors have three here)
            let tris: Vec<[usize; 3]> = match (corners.len(), reversed) {
                (4, (false, false)) if lattice_cell => vec![[0, 1, 3], [1, 2, 3]],
                (4, (false, true)) if lattice_cell => vec![[3, 2, 0], [2, 1, 0]],
                (4, (true, false)) if lattice_cell => vec![[1, 0, 2], [0, 3, 2]],
                (4, (true, true)) if lattice_cell => vec![[2, 3, 1], [3, 0, 1]],
                // a patch of an image: ABD, then BCD (streamed to the shader as a lattice of
                // two columns)
                (4, _) if image_metas.is_some() => vec![[0, 1, 3], [1, 2, 3]],
                _ => vec![],
            };
            for t in &tris {
                let paint = gouraud(&t.map(|k| (corners[k], metas[k])), cm, range, &fill);
                let segs = vec![Segment::MoveTo(corners[t[0]]), Segment::LineTo(corners[t[1]]), Segment::LineTo(corners[t[2]]), Segment::Close];
                if !e.canvas_frozen {
                    e.canvas.include_path(&segs);
                }
                e.canvas.push(Item::Path { segments: segs, fill: Some(Fill { paint, opacity: opacity.1, even_odd: false }), stroke: None });
            }
            if !tris.is_empty() {
                // `faceted interp`: the patch's edges on top, in `faceted color` (antialiased, as
                // lines are)
                if shader == "faceted interp" {
                    let mut segs = vec![Segment::MoveTo(corners[0])];
                    segs.extend(corners[1..].iter().map(|c| Segment::LineTo(*c)));
                    segs.push(Segment::Close);
                    let stroke = Stroke {
                        color: faceted.at(mapped),
                        width,
                        dash: st.dash.clone().filter(|d| !d.0.is_empty()),
                        cap: st.cap,
                        join: st.join,
                        miter_limit: st.miter_limit,
                        opacity: opacity.0,
                    };
                    e.canvas.push(Item::End);
                    e.canvas.push(Item::Path { segments: segs, fill: None, stroke: Some(stroke) });
                    e.canvas.push(Item::Crisp);
                }
                continue;
            }
        }
        // another quadrilateral (`patch type=bilinear`, a patch of a scattered mesh): its metas
        // bilinear across it, drawn as a grid of cells each split into two triangles shaded exactly
        // as above, so its colors change smoothly and in no bands
        if let Some(cm) = cmap.filter(|c| c.access == super::colormap::Access::Map && closes && shaded && vertex.is_none() && corners.len() == 4 && metas.len() == 4) {
            let size = (0..4).map(|i| corners[i].minus(corners[(i + 1) % 4]).norm()).fold(0.0, f64::max);
            let n = ((size / (6.0 * crate::PT)).ceil() as usize).clamp(1, 16);
            let nf = n as f64;
            let at = |u: f64, v: f64| {
                let w = [(1.0 - u) * (1.0 - v), u * (1.0 - v), u * v, (1.0 - u) * v];
                let pt = (0..4).fold(Point::default(), |acc, k| acc.plus(corners[k].scale(w[k])));
                (pt, (0..4).map(|k| metas[k] * w[k]).sum::<f64>())
            };
            // under the cells, the patch in its mean color: where the cells' edges meet, drawn
            // without antialiasing, a pixel no cell covers shows that and not what is behind
            let mut under = vec![Segment::MoveTo(corners[0])];
            under.extend(corners[1..].iter().map(|c| Segment::LineTo(*c)));
            under.push(Segment::Close);
            let mean_meta = metas.iter().sum::<f64>() / 4.0;
            let mean_color = fill.at(cm.color_of(mean_meta.clamp(range[0].min(range[1]), range[0].max(range[1])), range[0], range[1]));
            e.canvas.push(Item::Path { segments: under, fill: Some(Fill { paint: Paint::Solid(mean_color), opacity: opacity.1, even_odd: false }), stroke: None });
            for i in 0..n {
                for j in 0..n {
                    let (u0, u1, v0, v1) = (i as f64 / nf, (i + 1) as f64 / nf, j as f64 / nf, (j + 1) as f64 / nf);
                    let q = [at(u0, v0), at(u1, v0), at(u1, v1), at(u0, v1)];
                    for t in [[0, 1, 2], [0, 2, 3]] {
                        let tri = t.map(|k| q[k]);
                        let paint = gouraud(&tri, cm, range, &fill);
                        let segs = vec![Segment::MoveTo(tri[0].0), Segment::LineTo(tri[1].0), Segment::LineTo(tri[2].0), Segment::Close];
                        if !e.canvas_frozen {
                            e.canvas.include_path(&segs);
                        }
                        e.canvas.push(Item::Path { segments: segs, fill: Some(Fill { paint, opacity: opacity.1, even_odd: false }), stroke: None });
                    }
                }
            }
            if shader == "faceted interp" {
                let mut segs = vec![Segment::MoveTo(corners[0])];
                segs.extend(corners[1..].iter().map(|c| Segment::LineTo(*c)));
                segs.push(Segment::Close);
                let stroke = Stroke {
                    color: faceted.at(mapped),
                    width,
                    dash: st.dash.clone().filter(|d| !d.0.is_empty()),
                    cap: st.cap,
                    join: st.join,
                    miter_limit: st.miter_limit,
                    opacity: opacity.0,
                };
                e.canvas.push(Item::End);
                e.canvas.push(Item::Path { segments: segs, fill: None, stroke: Some(stroke) });
                e.canvas.push(Item::Crisp);
            }
            continue;
        }
        if closes && shader == "interp" && interp_ok && (corners.len() == 4 || corners.len() == 3) {
            let size = (0..corners.len()).map(|i| corners[i].minus(corners[(i + 1) % corners.len()]).norm()).fold(0.0, f64::max);
            let n = if size > 12.0 * crate::PT { ((size / (3.0 * crate::PT)).ceil() as usize).clamp(1, 64) } else { 1 };
            if n > 1 {
                // a point and its color from weights of the corners
                let at = |w: &[f64]| -> (Point, Color) {
                    let mut pt = Point::default();
                    for k in 0..corners.len() {
                        pt = pt.plus(corners[k].scale(w[k]));
                    }
                    let c = match &vertex {
                        Some(v) => blend(v, w),
                        None => map(f, (0..corners.len()).map(|k| metas[k] * w[k]).sum()),
                    };
                    (pt, c)
                };
                let mut pieces: Vec<Vec<(Point, Color)>> = Vec::new();
                let nf = n as f64;
                if corners.len() == 4 && p.lattice.is_some() && !p.image && kind != "bilinear" {
                    // a cell of a matrix is shaded as two triangles (a shading of type 5), ABD
                    // then BCD of its corners as the z buffer has turned them (reversing the
                    // rows or the columns)
                    let tris: [[usize; 3]; 2] = match reversed {
                        (false, false) => [[0, 1, 3], [1, 2, 3]],
                        (false, true) => [[3, 2, 0], [2, 1, 0]],
                        (true, false) => [[1, 0, 2], [0, 3, 2]],
                        (true, true) => [[2, 3, 1], [3, 0, 1]],
                    };
                    for t in tris {
                        let bar = |i: usize, j: usize| {
                            let (a, b) = (i as f64 / nf, j as f64 / nf);
                            let mut w = [0.0; 4];
                            w[t[0]] = 1.0 - a - b;
                            w[t[1]] = a;
                            w[t[2]] = b;
                            w
                        };
                        for i in 0..n {
                            for j in 0..n - i {
                                pieces.push([bar(i, j), bar(i + 1, j), bar(i, j + 1)].iter().map(|w| at(w)).collect());
                                if i + j + 1 < n {
                                    pieces.push([bar(i + 1, j), bar(i + 1, j + 1), bar(i, j + 1)].iter().map(|w| at(w)).collect());
                                }
                            }
                        }
                    }
                } else if corners.len() == 4 {
                    let bil = |u: f64, v: f64| [(1.0 - u) * (1.0 - v), u * (1.0 - v), u * v, (1.0 - u) * v];
                    for i in 0..n {
                        for j in 0..n {
                            let (u0, u1, v0, v1) = (i as f64 / nf, (i + 1) as f64 / nf, j as f64 / nf, (j + 1) as f64 / nf);
                            pieces.push([bil(u0, v0), bil(u1, v0), bil(u1, v1), bil(u0, v1)].iter().map(|w| at(w)).collect());
                        }
                    }
                } else {
                    let bar = |i: usize, j: usize| {
                        let (a, b) = (i as f64 / nf, j as f64 / nf);
                        [1.0 - a - b, a, b]
                    };
                    for i in 0..n {
                        for j in 0..n - i {
                            pieces.push([bar(i, j), bar(i + 1, j), bar(i, j + 1)].iter().map(|w| at(w)).collect());
                            if i + j + 1 < n {
                                pieces.push([bar(i + 1, j), bar(i + 1, j + 1), bar(i, j + 1)].iter().map(|w| at(w)).collect());
                            }
                        }
                    }
                }
                for piece in pieces {
                    let c = fill.at(mean(&piece.iter().map(|q| q.1).collect::<Vec<_>>()));
                    let mut segs = vec![Segment::MoveTo(piece[0].0)];
                    segs.extend(piece[1..].iter().map(|q| Segment::LineTo(q.0)));
                    segs.push(Segment::Close);
                    if !e.canvas_frozen {
                        e.canvas.include_path(&segs);
                    }
                    let stroke = Stroke { color: c, width: 0.5 * crate::PT, dash: None, cap: st.cap, join: st.join, miter_limit: st.miter_limit, opacity: opacity.0 };
                    e.canvas.push(Item::Path { segments: segs, fill: Some(Fill { paint: Paint::Solid(c), opacity: opacity.1, even_odd: false }), stroke: Some(stroke) });
                }
                continue;
            }
        }
        let mut segs = vec![Segment::MoveTo(corners[0])];
        segs.extend(corners[1..].iter().map(|c| Segment::LineTo(*c)));
        if closes {
            segs.push(Segment::Close);
        }
        if !e.canvas_frozen {
            e.canvas.include_path(&segs);
        }
        let stroke =
            |c: Color| Stroke { color: c, width, dash: st.dash.clone().filter(|d| !d.0.is_empty()), cap: st.cap, join: st.join, miter_limit: st.miter_limit, opacity: opacity.0 };
        let (fill_c, stroke_c) = if closes && (surf || p.patch) {
            match shader.as_str() {
                // pgfplots shades each patch; flat patches with a hairline of their color have
                // no seams between them either
                "interp" => (Some(fill.at(mapped)), Some(fill.at(mapped))),
                "flat" | "flat mean" | "flat corner" => (Some(fill.at(mapped)), Some(draw.at(mapped))),
                _ => (Some(fill.at(mapped)), Some(faceted.at(mapped))),
            }
        } else {
            (None, Some(draw.at(mapped)))
        };
        let fill_item = fill_c.map(|c| Fill { paint: Paint::Solid(c), opacity: opacity.1, even_odd: false });
        let mut stroke_item = stroke_c.map(stroke);
        if let (Some(st), true) = (stroke_item.as_mut(), surf && closes && shader == "interp") {
            st.width = 0.2 * crate::PT;
        }
        e.canvas.push(Item::Path { segments: segs, fill: fill_item, stroke: stroke_item });
    }
    if crisp {
        e.canvas.push(Item::End);
    }
    Ok(())
}

/// The single path of one dimensional patches without point meta (`supports global path`):
/// their curves one after the other, joined where they meet. None for other plots.
pub fn global_path(e: &mut Engine, p: &Plot, pts: &[[f64; 3]], f: &Frame) -> Option<Vec<Segment>> {
    use super::patch::{Edge, Kind, Patch};
    if !p.patch || p.lattice.is_some() || p.has_meta {
        return None;
    }
    let kind = Kind::from_name(&keys::value(e, "/pgfplots/patch type"), 4);
    if !kind.is_curve() {
        return None;
    }
    let mut segs = Vec::new();
    let mut last: Option<Point> = None;
    for chunk in pts.chunks(kind.size()).filter(|c| c.len() == kind.size()) {
        let patch = Patch { kind, pts: chunk.to_vec(), metas: vec![f64::NAN; kind.size()] };
        for edge in patch.outline() {
            let (a, b, seg) = match edge {
                Edge::Line(a, b) => (f.at(a), f.at(b), Segment::LineTo(f.at(b))),
                Edge::Curve(a, c1, c2, b) => (f.at(a), f.at(b), Segment::CurveTo(f.at(c1), f.at(c2), f.at(b))),
            };
            if last.is_none_or(|q| q.minus(a).norm() > 1e-9) {
                segs.push(Segment::MoveTo(a));
            }
            last = Some(b);
            segs.push(seg);
        }
    }
    Some(segs)
}

/// The range of a plot's point meta (`point meta rel=per plot`).
pub fn plot_meta_range(p: &Plot) -> Option<[f64; 2]> {
    let (lo, hi) = p.coords.iter().map(|c| c.meta).filter(|m| m.is_finite()).fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), m| (lo.min(m), hi.max(m)));
    (lo.is_finite() && hi > lo).then_some([lo, hi])
}

/// How a patch plot looks: its colors as functions of `mapped color`, its shader and stroke.
struct Look {
    draw: Linear,
    fill: Linear,
    faceted: Linear,
    shader: String,
    stroke: super::super::GState,
    /// `surf` or `patch` (filled), not `mesh`.
    surf: bool,
}

/// The depth of logical points along the view (larger is nearer), as the z buffer sorts: the
/// scalar product with the direction the axis' unit vectors flatten
/// (`\pgfplotsgetnormalforcurrentview`), in the coordinates of the data scale transformation.
fn depth_fn(e: &mut Engine, f: &Frame) -> impl Fn(&[[f64; 3]]) -> f64 {
    let (n, s) = toward_viewer(e, f);
    let c = [0, 1, 2].map(|i| n[i] * s[i]);
    move |pts: &[[f64; 3]]| pts.iter().map(|l| (0..3).map(|i| if c[i] == 0.0 { 0.0 } else { c[i] * l[i] }).sum::<f64>()).sum::<f64>() / pts.len().max(1) as f64
}

/// The direction toward the viewer in the coordinates of the data scale transformation, and the
/// transformation's factors.
fn toward_viewer(e: &mut Engine, f: &Frame) -> ([f64; 3], [f64; 3]) {
    let az = keys::number(e, "/pgfplots/view/az").unwrap_or(25.0).to_radians();
    let el = keys::number(e, "/pgfplots/view/el").unwrap_or(30.0).to_radians();
    let d = [az.sin() * el.cos(), -az.cos() * el.cos(), el.sin()];
    let s = [0, 1, 2].map(|i| super::draw::data_scale_trafo(e, f, i).0);
    let cross = |a: Point, b: Point| a.x * b.y - a.y * b.x;
    // the unit vectors of the transformed coordinates, and the direction they flatten
    let t = [0, 1, 2].map(|i| f.e[i].scale(1.0 / s[i]));
    let mut n = [cross(t[1], t[2]), cross(t[2], t[0]), cross(t[0], t[1])];
    // nearer is where `view={az}{el}` looks from: the sign of the flattened logical direction
    let u = [cross(f.e[1], f.e[2]), cross(f.e[2], f.e[0]), cross(f.e[0], f.e[1])];
    let toward: f64 = (0..3).map(|i| d[i] * u[i] / (f.max[i] - f.min[i]).abs().max(1e-300)).sum();
    let along: f64 = (0..3).map(|i| n[i] * s[i] * u[i]).sum();
    if toward * along < 0.0 {
        n = n.map(|v| -v);
    }
    (n, s)
}

/// `mesh/interior colormap`: whether a patch with corners `a`, `b` (next in x) and `d` (next in
/// y) shows its inside, its normal (times `sign`) at least `thresh` (a cosine) away from the
/// viewer.
fn interior_fn(e: &mut Engine, f: &Frame, thresh: f64) -> impl Fn([f64; 3], [f64; 3], [f64; 3], f64) -> bool {
    let (n, s) = toward_viewer(e, f);
    move |a: [f64; 3], b: [f64; 3], d: [f64; 3], sign: f64| {
        let u = [0, 1, 2].map(|i| (b[i] - a[i]) * s[i]);
        let v = [0, 1, 2].map(|i| (d[i] - a[i]) * s[i]);
        let normal = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let dot: f64 = (0..3).map(|i| -n[i] * normal[i] * sign).sum();
        let len = (n.iter().map(|x| x * x).sum::<f64>() * normal.iter().map(|x| x * x).sum::<f64>()).sqrt();
        if thresh == 0.0 {
            dot < 0.0
        } else {
            dot / len.max(1e-300) < thresh
        }
    }
}

/// `\addplot [patch, patch type=…]` from a list of patches (`mesh input=patches`): each patch
/// refined (`patch refines`) and cut in triangles (`patch to triangles`) as asked, sorted back to
/// front in 3d, then filled with one color (`flat`, `faceted`) or shaded (`interp`), its outline
/// drawn (`faceted`, `faceted interp`, a `mesh`); a curve (`line`, the splines) is drawn.
fn draw_patches(e: &mut Engine, p: &Plot, pts: &[[f64; 3]], f: &Frame, kind: &str, look: &Look) -> Result<(), Error> {
    use super::patch::{Edge, Kind, Patch};
    let count = keys::number(e, "/pgfplots/vertex count").unwrap_or(4.0).max(1.0) as usize;
    let kind = Kind::from_name(kind, count);
    let refines = keys::number(e, "/pgfplots/patch refines").unwrap_or(0.0).clamp(0.0, 6.0) as usize;
    let to_triangles = keys::flag(e, "/pgfplots/patch to triangles");
    let dims = if f.three_d { 3 } else { 2 };
    let mut list: Vec<Patch> = Vec::new();
    for (i, chunk) in pts.chunks(kind.size()).enumerate() {
        if chunk.len() < kind.size() || chunk.iter().any(|l| l.iter().take(dims).any(|v| !v.is_finite())) {
            continue;
        }
        let metas: Vec<f64> = (0..kind.size()).map(|k| p.coords.get(i * kind.size() + k).map_or(f64::NAN, |c| c.meta)).collect();
        list.push(Patch { kind, pts: chunk.to_vec(), metas });
    }
    for _ in 0..refines {
        list = list.iter().flat_map(Patch::refine).collect();
    }
    if to_triangles {
        list = list.iter().flat_map(Patch::triangulate).collect();
    }
    let zbuffer = keys::value(e, "/pgfplots/z buffer");
    let depth = depth_fn(e, f);
    if f.three_d && zbuffer != "none" {
        list.sort_by(|a, b| depth(&a.pts).total_cmp(&depth(&b.pts)));
    }
    let st = &look.stroke;
    let corner = look.shader == "flat corner";
    let stroke_of = |c: Color, width: f64| Stroke {
        color: c,
        width,
        dash: st.dash.clone().filter(|d| !d.0.is_empty()),
        cap: st.cap,
        join: st.join,
        miter_limit: st.miter_limit,
        opacity: st.draw_opacity,
    };
    let path = |edges: &[Edge], close: bool| -> Vec<Segment> {
        let mut segs = Vec::new();
        for (k, edge) in edges.iter().enumerate() {
            let (a, rest): (_, Vec<Point>) = match *edge {
                Edge::Line(a, b) => (a, vec![f.at(b)]),
                Edge::Curve(a, c1, c2, b) => (a, vec![f.at(c1), f.at(c2), f.at(b)]),
            };
            if k == 0 {
                segs.push(Segment::MoveTo(f.at(a)));
            }
            segs.push(if rest.len() == 1 { Segment::LineTo(rest[0]) } else { Segment::CurveTo(rest[0], rest[1], rest[2]) });
        }
        if close && !segs.is_empty() {
            segs.push(Segment::Close);
        }
        segs
    };
    // how finely a shaded patch is cut: the same for all of the plot's, so that two patches cut a
    // side they share at the same points and leave no crack between them
    let shade_cuts = list
        .iter()
        .map(|patch| {
            let c: Vec<Point> = patch.pts.iter().map(|l| f.at(*l)).collect();
            let (lo, hi) = c.iter().fold((c[0], c[0]), |(lo, hi), q| (Point::new(lo.x.min(q.x), lo.y.min(q.y)), Point::new(hi.x.max(q.x), hi.y.max(q.y))));
            ((hi.x - lo.x).max(hi.y - lo.y) / (6.0 * crate::PT)).ceil() as usize
        })
        .max()
        .unwrap_or(1);
    for patch in list {
        let m = patch.flat_meta(corner);
        // a patch whose color comes from a point meta of nan is not drawn
        if p.has_meta && !m.is_finite() {
            continue;
        }
        let mapped = if m.is_finite() { map(f, m) } else { Color::BLACK };
        let curve = kind.is_curve();
        let outline = path(&patch.outline(), !curve);
        if !e.canvas_frozen {
            e.canvas.include_path(&outline);
        }
        if curve || !look.surf {
            e.canvas.push(Item::Path { segments: outline, fill: None, stroke: Some(stroke_of(look.draw.at(mapped), st.line_width)) });
            continue;
        }
        let interp = look.shader.contains("interp");
        if interp && patch.metas.iter().all(|m| m.is_finite()) {
            // the shading: small pieces of the patch, each of the color at its middle;
            // a rectangle is shaded as two triangles
            let parts = if kind == Kind::Rectangle { patch.triangulate() } else { vec![patch.clone()] };
            e.canvas.push(Item::Crisp);
            for part in parts {
                // a triangle: its linear gradient, exact
                if let Some(cm) = f.colormap.as_ref().filter(|c| c.access == super::colormap::Access::Map && part.kind == Kind::Triangle) {
                    let c: Vec<Point> = part.pts.iter().map(|l| f.at(*l)).collect();
                    let paint = gouraud(&[(c[0], part.metas[0]), (c[1], part.metas[1]), (c[2], part.metas[2])], cm, f.meta, &look.fill);
                    let segs = vec![Segment::MoveTo(c[0]), Segment::LineTo(c[1]), Segment::LineTo(c[2]), Segment::Close];
                    e.canvas.push(Item::Path { segments: segs, fill: Some(Fill { paint, opacity: st.fill_opacity, even_odd: false }), stroke: None });
                    continue;
                }
                let size = {
                    let c: Vec<Point> = part.pts.iter().map(|l| f.at(*l)).collect();
                    let (lo, hi) = c.iter().fold((c[0], c[0]), |(lo, hi), q| (Point::new(lo.x.min(q.x), lo.y.min(q.y)), Point::new(hi.x.max(q.x), hi.y.max(q.y))));
                    (hi.x - lo.x).max(hi.y - lo.y)
                };
                let curved = !matches!(part.kind, Kind::Triangle | Kind::Bilinear | Kind::Rectangle | Kind::Polygon(_));
                // a continuous colormap: small triangles, each its exact linear gradient, over the
                // patch in its mean color (a pixel no triangle covers, where their edges meet
                // without antialiasing, shows that)
                // (not a quadratic triangle: pgfplots draws it as a Coons patch, its inside not the
                // isoparametric one `eval` gives, where a fold shows it)
                if let Some(cm) = f.colormap.as_ref().filter(|c| c.access == super::colormap::Access::Map && !matches!(part.kind, Kind::Polygon(_) | Kind::QuadTriangle)) {
                    let n = shade_cuts.clamp(if curved { 6 } else { 1 }, 24);
                    let mut tris = part.shaded_triangles(n);
                    if f.three_d {
                        tris.sort_by(|a, b| depth(&a.iter().map(|t| t.0).collect::<Vec<_>>()).total_cmp(&depth(&b.iter().map(|t| t.0).collect::<Vec<_>>())));
                    }
                    let mean = part.metas.iter().sum::<f64>() / part.metas.len().max(1) as f64;
                    let rim = part.shaded_outline(n);
                    let mut under = vec![Segment::MoveTo(f.at(rim[0]))];
                    under.extend(rim[1..].iter().map(|q| Segment::LineTo(f.at(*q))));
                    under.push(Segment::Close);
                    e.canvas.push(Item::Path {
                        segments: under,
                        fill: Some(Fill { paint: Paint::Solid(look.fill.at(map(f, mean))), opacity: st.fill_opacity, even_odd: false }),
                        stroke: None,
                    });
                    for t in tris {
                        let c = t.map(|(q, m)| (f.at(q), m));
                        let paint = gouraud(&c, cm, f.meta, &look.fill);
                        let segs = vec![Segment::MoveTo(c[0].0), Segment::LineTo(c[1].0), Segment::LineTo(c[2].0), Segment::Close];
                        e.canvas.push(Item::Path { segments: segs, fill: Some(Fill { paint, opacity: st.fill_opacity, even_odd: false }), stroke: None });
                    }
                    continue;
                }
                let n = ((size / (3.0 * crate::PT)).ceil() as usize).clamp(if curved { 6 } else { 1 }, 48);
                let mut pieces = part.pieces(n);
                if f.three_d {
                    pieces.sort_by(|a, b| depth(&a.0).total_cmp(&depth(&b.0)));
                }
                for (q, meta) in pieces {
                    let c = look.fill.at(map(f, meta));
                    let mut segs = vec![Segment::MoveTo(f.at(q[0]))];
                    segs.extend(q[1..].iter().map(|l| Segment::LineTo(f.at(*l))));
                    segs.push(Segment::Close);
                    let stroke = Stroke { color: c, width: 0.5 * crate::PT, dash: None, cap: st.cap, join: st.join, miter_limit: st.miter_limit, opacity: st.draw_opacity };
                    e.canvas.push(Item::Path { segments: segs, fill: Some(Fill { paint: Paint::Solid(c), opacity: st.fill_opacity, even_odd: false }), stroke: Some(stroke) });
                }
            }
            e.canvas.push(Item::End);
            if look.shader == "faceted interp" {
                e.canvas.push(Item::Path { segments: outline, fill: None, stroke: Some(stroke_of(look.faceted.at(mapped), st.line_width)) });
            }
            continue;
        }
        let edge = if look.shader.starts_with("flat") {
            look.draw.at(mapped)
        } else if interp {
            look.fill.at(mapped)
        } else {
            look.faceted.at(mapped)
        };
        e.canvas.push(Item::Path {
            segments: outline,
            fill: Some(Fill { paint: Paint::Solid(look.fill.at(mapped)), opacity: st.fill_opacity, even_odd: false }),
            stroke: Some(stroke_of(edge, st.line_width)),
        });
    }
    Ok(())
}

/// A triangle of `shader=interp`: pgfplots hands the point metas to a Gouraud shading (type 4)
/// whose function is the colormap, so the meta is affine across the triangle and the color
/// changes along the meta's gradient only. That is a linear gradient, exact: the colormap's own
/// stops between the triangle's lowest and highest meta.
fn gouraud(tri: &[(Point, f64); 3], cm: &super::colormap::Colormap, range: [f64; 2], fill: &Linear) -> Paint {
    let color = |m: f64| fill.at(cm.color_of(m, range[0], range[1]));
    // a meta out of the range is taken into it at the corner (the shading's domain), before the
    // shading interpolates
    let tri = &tri.map(|(q, m)| (q, m.clamp(range[0].min(range[1]), range[1].max(range[0]))));
    let (p0, m0) = tri[0];
    let (lo, hi) = tri.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), t| (lo.min(t.1), hi.max(t.1)));
    let (e1, e2) = (tri[1].0.minus(p0), tri[2].0.minus(p0));
    let det = e1.x * e2.y - e1.y * e2.x;
    if det.abs() < 1e-12 || hi - lo <= 1e-12 * (lo.abs() + hi.abs() + 1.0) {
        return Paint::Solid(color((tri[0].1 + tri[1].1 + tri[2].1) / 3.0));
    }
    // the gradient g of the meta on the canvas: e1·g = d1, e2·g = d2
    let (d1, d2) = (tri[1].1 - m0, tri[2].1 - m0);
    let g = Point::new((d1 * e2.y - d2 * e1.y) / det, (e1.x * d2 - e2.x * d1) / det);
    let g2 = g.x * g.x + g.y * g.y;
    // from the point of the lowest meta to that of the highest, along g
    let a = p0.plus(g.scale((lo - m0) / g2));
    let u = g.scale((hi - lo) / g2);
    let transform = crate::scene::Transform { a: -u.y / 100.0, b: u.x / 100.0, c: u.x / 100.0, d: u.y / 100.0, e: a.x + 0.5 * u.x, f: a.y + 0.5 * u.y };
    let mut metas = vec![lo, hi];
    for (t, _) in cm.normalized() {
        let m = range[0] + t * (range[1] - range[0]);
        if m > lo && m < hi {
            metas.push(m);
        }
    }
    metas.sort_by(f64::total_cmp);
    let stops = metas.iter().map(|&m| ((m - lo) / (hi - lo), color(m))).collect();
    Paint::Shading(crate::canvas::Shading::Axial { transform, stops })
}

fn mean(v: &[Color]) -> Color {
    let n = v.len().max(1) as f64;
    Color::rgb(v.iter().map(|c| c.r).sum::<f64>() / n, v.iter().map(|c| c.g).sum::<f64>() / n, v.iter().map(|c| c.b).sum::<f64>() / n)
}

fn blend(v: &[Color], w: &[f64]) -> Color {
    let mut c = Color::rgb(0.0, 0.0, 0.0);
    for (k, x) in v.iter().zip(w) {
        c.r += k.r * x;
        c.g += k.g * x;
        c.b += k.b * x;
    }
    c
}

/// The color of point `k` of an explicit color input: `color=red`, `rgb=…`, `R,G,B`, `rgb255=`,
/// `cmyk=`, `gray=`, `wave=` (nanometres), as written or computed by the survey; None for `[]`.
fn vertex_color(e: &Engine, p: &Plot, k: usize) -> Option<Color> {
    let Some(label) = p.coords.get(k).filter(|c| c.label != super::plot::NO_LABEL).and_then(|c| p.labels.get(c.label as usize)) else { return Some(Color::BLACK) };
    let s = crate::tex::tokens_to_string(label);
    if s.trim().is_empty() {
        return None;
    }
    Some(explicit_color(e, &s).unwrap_or(Color::BLACK))
}

pub fn explicit_color(e: &Engine, s: &str) -> Option<Color> {
    let s = s.trim();
    let (model, rest) = match s.split_once('=') {
        Some((m, r)) => (m.trim(), r.trim()),
        None => ("rgb", s),
    };
    // (values of \thisrow come in parentheses)
    let nums: Vec<f64> = rest.split(',').filter_map(|x| x.trim().trim_start_matches('(').trim_end_matches(')').trim().parse().ok()).collect();
    let n = |i: usize| nums.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
    Some(match model {
        "color" => return e.color(rest).map(|c| c.to_rgb()),
        "rgb" => Color::rgb(n(0), n(1), n(2)),
        "rgb255" => Color::rgb(nums.first()? / 255.0, nums.get(1)? / 255.0, nums.get(2)? / 255.0),
        "gray" => Color::rgb(n(0), n(0), n(0)),
        "cmyk" => Color::rgb((1.0 - n(0)) * (1.0 - n(3)), (1.0 - n(1)) * (1.0 - n(3)), (1.0 - n(2)) * (1.0 - n(3))),
        "cmy" => Color::rgb(1.0 - n(0), 1.0 - n(1), 1.0 - n(2)),
        "wave" => wave(*nums.first()?),
        _ => return None,
    })
}

/// xcolor's `wave` model: a wavelength in nm to RGB (Bruton's approximation, as xcolor does).
fn wave(l: f64) -> Color {
    let (r, g, b) = if l < 440.0 {
        ((440.0 - l) / 60.0, 0.0, 1.0)
    } else if l < 490.0 {
        (0.0, (l - 440.0) / 50.0, 1.0)
    } else if l < 510.0 {
        (0.0, 1.0, (510.0 - l) / 20.0)
    } else if l < 580.0 {
        ((l - 510.0) / 70.0, 1.0, 0.0)
    } else if l < 645.0 {
        (1.0, (645.0 - l) / 65.0, 0.0)
    } else {
        (1.0, 0.0, 0.0)
    };
    let f = if l < 420.0 {
        0.3 + 0.7 * (l - 380.0) / 40.0
    } else if l > 700.0 {
        0.3 + 0.7 * (780.0 - l) / 80.0
    } else {
        1.0
    };
    let c = |v: f64| (v * f).clamp(0.0, 1.0).powf(0.8);
    Color::rgb(c(r), c(g), c(b))
}
