//! Prepared contours (`contour prepared`, the contour plot handler of `pgfplotsplothandlers.code.tex`):
//! the input is already the contour lines, one level per point meta (z in 3d, the third column in
//! 2d), broken at empty lines. Each line is stroked in `contour/draw color` (the colormap's color of
//! its level); labels with the level go every `contour/label distance` along it, the first one
//! sooner, a line too short for one still gets one on its last segment.

use crate::scene::{Point, Segment};
use crate::Error;

use super::super::nodes::{NodeSpec, OnPath};
use super::draw::Frame;
use super::keys;
use super::plot::{Coord, Plot};
use super::{toks, Engine};

/// `contour prepared format=matlab`: `(level, count)` headers each followed by `count` points.
pub fn from_matlab(plot: &mut Plot) {
    let mut out: Vec<Coord> = Vec::new();
    let mut scanlines = Vec::new();
    let mut i = 0;
    while i < plot.coords.len() {
        let head = plot.coords[i];
        let (level, count) = (head.x, head.y.max(0.0) as usize);
        i += 1;
        if !out.is_empty() {
            scanlines.push(out.len());
        }
        for c in plot.coords.iter().skip(i).take(count) {
            let mut c = *c;
            c.meta = level;
            c.z = level;
            out.push(c);
        }
        i += count;
    }
    plot.coords = out;
    plot.scanlines = scanlines;
    plot.has_meta = true;
}

/// The lines of a contour plot: runs of points of one level, not across empty lines.
fn runs(p: &Plot, pts: &[[f64; 3]]) -> Vec<(f64, Vec<usize>)> {
    let mut out: Vec<(f64, Vec<usize>)> = Vec::new();
    for (i, l) in pts.iter().enumerate() {
        let meta = p.coords.get(i).map_or(f64::NAN, |c| c.meta);
        if !(l[0].is_finite() && l[1].is_finite()) {
            out.push((f64::NAN, Vec::new()));
            continue;
        }
        let jump = p.scanlines.contains(&i);
        match out.last_mut() {
            Some((m, v)) if !jump && !v.is_empty() && (*m == meta || (m.is_nan() && meta.is_nan())) => v.push(i),
            _ => out.push((meta, vec![i])),
        }
    }
    out.retain(|(_, v)| !v.is_empty());
    out
}

/// Draws the contour lines and their labels (in the plot's group, its style set).
pub fn draw(e: &mut Engine, p: &Plot, pts: &[[f64; 3]], f: &Frame) -> Result<(), Error> {
    let draw_color = keys::value(e, "/pgfplots/contour/draw color");
    let fill_color = keys::value(e, "/pgfplots/contour/fill color");
    let filled = keys::flag(e, "/pgfplots/contour/filled");
    let labels = keys::flag(e, "/pgfplots/contour/labels");
    let dist = keys::length(e, "/pgfplots/contour/label distance").unwrap_or(70.0 * crate::PT);
    let mut pending: Vec<(Point, Point, f64)> = Vec::new();
    for (meta, run) in runs(p, pts) {
        let canvas: Vec<Point> = run.iter().map(|&i| f.at(pts[i])).collect();
        let mut segs = vec![Segment::MoveTo(canvas[0])];
        segs.extend(canvas[1..].iter().map(|c| Segment::LineTo(*c)));
        if filled {
            segs.push(Segment::Close);
        }
        e.begin_group();
        let r = (|| -> Result<(), Error> {
            if meta.is_finite() && f.colormap.is_some() {
                e.colors.insert("mapped color".into(), super::mesh::map(f, meta));
            }
            let mut opts = format!("/tikz/draw={draw_color}");
            if filled {
                opts.push_str(&format!(",/tikz/fill={fill_color}"));
            }
            e.st.mode.draw = true;
            e.process_keys(toks(&opts), Some("/tikz"))?;
            e.st.mode.fill = filled;
            e.use_path(segs, "addplot")
        })();
        e.end_group();
        r?;
        // labels: every `label distance`, the first after a fifth of it
        if labels && canvas.len() >= 2 {
            let mut len = 0.2 * dist;
            let mut has = false;
            for w in canvas.windows(2) {
                len += w[1].minus(w[0]).norm();
                if len > dist {
                    pending.push((w[0], w[1], meta));
                    has = true;
                    len -= dist;
                }
            }
            if !has {
                let n = canvas.len();
                pending.push((canvas[n - 2], canvas[n - 1], meta));
            }
        }
    }
    e.colors.insert("mapped color".into(), crate::color::Color::BLACK);
    if pending.is_empty() {
        return Ok(());
    }
    // the labels, in `every contour label`
    e.begin_group();
    let r = (|| -> Result<(), Error> {
        // (the axis' meta range is never empty: an exponent of 0 for a range at 0 is a guard)
        let exp = Some(f.meta[0].abs().max(f.meta[1].abs()).log10().floor()).filter(|x| x.is_finite()).unwrap_or(0.0);
        e.tex.set_meaning(&crate::tex::Token::cs("pgfplotspointmetarangeexponent"), super::super::macro_from(&super::super::pgf_number(exp)), false);
        e.st.key_path = "/tikz".into();
        e.process_keys(toks("/pgfplots/contour/every contour label"), Some("/tikz"))?;
        for (a, b, meta) in pending {
            e.begin_group();
            let r = (|| -> Result<(), Error> {
                if meta.is_finite() && f.colormap.is_some() {
                    e.colors.insert("mapped color".into(), super::mesh::map(f, meta));
                }
                let at = Point::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
                // along the segment, never upside down
                let d = b.minus(a);
                let mut angle = d.y.atan2(d.x).to_degrees();
                if d.x < 0.0 {
                    angle += 180.0;
                }
                let text = super::draw::print_numbers(e, &toks(&format!("\\pgfmathprintnumber{{{}}}", super::super::pgf_number(meta))))?;
                let options = toks(&format!("rotate={angle}"));
                let items = e.node(NodeSpec { options, name: None, at: Some(at), text, place: OnPath::Point(at), point: at, coordinate: false, default_pos: None })?;
                e.canvas.items.extend(items);
                Ok(())
            })();
            e.colors.insert("mapped color".into(), crate::color::Color::BLACK);
            e.end_group();
            r?;
        }
        Ok(())
    })();
    e.end_group();
    r
}

/// The bands of `contour filled`: the level where each band begins and its color
/// (`contour/levels`, `levels from colormap`, or `number` colors of the colormap over the meta range).
fn bands(e: &mut Engine, f: &Frame) -> Vec<(f64, crate::color::Color)> {
    let (lo, hi) = (f.meta[0], f.meta[1]);
    let Some(cm) = f.colormap.clone() else { return Vec::new() };
    let levels = keys::value(e, "/pgfplots/contour/levels");
    if !levels.is_empty() {
        let mut v: Vec<f64> = levels.split(',').filter_map(|x| e.number(x.trim()).ok()).collect();
        v.sort_by(f64::total_cmp);
        return v.into_iter().map(|l| (l, super::mesh::map(f, l))).collect();
    }
    let from = keys::value(e, "/pgfplots/contour/levels from colormap");
    if !from.is_empty() {
        if let Some(m) = super::colormap::Colormap::parse(&from, &e.colors) {
            return m.stops.to_vec();
        }
    }
    let n = keys::number(e, "/pgfplots/contour/number").unwrap_or(5.0).max(1.0) as usize;
    (0..n).map(|i| (lo + (hi - lo) * i as f64 / n as f64, cm.at(if n > 1 { i as f64 / (n - 1) as f64 } else { 0.0 }))).collect()
}

/// The part of a triangle where its (linear) value is ≥ `lo` and ≤ `hi`.
pub fn clip(tri: &[(Point, f64)], lo: f64, hi: f64) -> Vec<Point> {
    fn cut(poly: Vec<(Point, f64)>, keep: &dyn Fn(f64) -> bool, at: f64) -> Vec<(Point, f64)> {
        let mut out = Vec::new();
        for i in 0..poly.len() {
            let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
            if keep(a.1) {
                out.push(a);
            }
            if keep(a.1) != keep(b.1) && (b.1 - a.1).abs() > 0.0 {
                let t = (at - a.1) / (b.1 - a.1);
                out.push((Point::new(a.0.x + t * (b.0.x - a.0.x), a.0.y + t * (b.0.y - a.0.y)), at));
            }
        }
        out
    }
    let p = if lo.is_finite() { cut(tri.to_vec(), &|m| m >= lo, lo) } else { tri.to_vec() };
    let p = if hi.is_finite() { cut(p, &|m| m <= hi, hi) } else { p };
    p.into_iter().map(|(q, _)| q).collect()
}

/// `contour filled`: the mesh's cells cut into bands of one color each.
pub fn draw_filled(e: &mut Engine, p: &Plot, pts: &[[f64; 3]], f: &Frame) -> Result<(), Error> {
    let Some((rows, cols)) = p.lattice else {
        e.warn("contour filled needs a matrix of points (an expression or a table with scanlines): not drawn");
        return Ok(());
    };
    let bands = bands(e, f);
    if bands.is_empty() {
        return Ok(());
    }
    let meta = |k: usize| p.coords.get(k).map_or(f64::NAN, |c| c.meta);
    let opacity = e.st.fill_opacity;
    // per band, all its pieces in one path
    let mut pieces: Vec<Vec<Segment>> = vec![Vec::new(); bands.len()];
    let reversed = if f.three_d {
        let az = keys::number(e, "/pgfplots/view/az").unwrap_or(25.0).to_radians();
        let el = keys::number(e, "/pgfplots/view/el").unwrap_or(30.0).to_radians();
        (az.sin() * el.cos() < -1e-9, -az.cos() * el.cos() < -1e-9)
    } else {
        (false, false)
    };
    for r in 0..rows.saturating_sub(1) {
        for c in 0..cols.saturating_sub(1) {
            let k = r * cols + c;
            let q = [k, k + 1, k + cols + 1, k + cols];
            if q.iter().any(|&i| i >= pts.len() || !meta(i).is_finite()) {
                continue;
            }
            // the cell's two triangles as the shading of type 5 cuts them: along the diagonal from
            // its second to its last corner, the corners turned as the z buffer reverses a 3d
            // matrix's rows or columns
            let tris = match reversed {
                (false, false) | (true, true) => [[q[0], q[1], q[3]], [q[1], q[2], q[3]]],
                _ => [[q[0], q[1], q[2]], [q[0], q[2], q[3]]],
            };
            for tri in tris {
                let t: Vec<(Point, f64)> = tri.iter().map(|&i| (f.at(pts[i]), meta(i))).collect();
                for (b, (level, _)) in bands.iter().enumerate() {
                    // the lowest band also takes what is below its level
                    let lo = if b == 0 { f64::NEG_INFINITY } else { *level };
                    let hi = bands.get(b + 1).map_or(f64::INFINITY, |n| n.0);
                    let poly = clip(&t, lo, hi);
                    if poly.len() >= 3 {
                        pieces[b].push(Segment::MoveTo(poly[0]));
                        pieces[b].extend(poly[1..].iter().map(|q| Segment::LineTo(*q)));
                        pieces[b].push(Segment::Close);
                    }
                }
            }
        }
    }
    for (segs, (_, color)) in pieces.into_iter().zip(bands) {
        if segs.is_empty() {
            continue;
        }
        if !e.canvas_frozen {
            e.canvas.include_path(&segs);
        }
        // a hairline of the band's color hides the seams between the pieces
        let stroke = crate::canvas::Stroke { color, width: 0.3 * crate::PT, dash: None, cap: e.st.cap, join: e.st.join, miter_limit: e.st.miter_limit, opacity };
        let fill = crate::canvas::Fill { paint: crate::canvas::Paint::Solid(color), opacity, even_odd: false };
        e.canvas.push(crate::canvas::Item::Path { segments: segs, fill: Some(fill), stroke: Some(stroke) });
    }
    Ok(())
}
