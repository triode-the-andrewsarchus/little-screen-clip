//! Box plots (`tikzlibrarypgfplots.statistics.code.tex`): `boxplot` computes the quartiles, the
//! median, the average and the whiskers of the plot's values (the outliers stay as its points),
//! `boxplot prepared` takes them as given. A box is drawn at `draw position` (`1+\plotnumofactualtype`)
//! across the values' direction (`draw direction=x`: values along x), `box extend` wide, the
//! whiskers `whisker extend` wide, as `boxplot/draw/…` draw them.

use crate::scene::{Point, Segment};
use crate::Error;

use super::draw::Frame;
use super::keys;
use super::plot::{Coord, Plot};
use super::Engine;

/// The numbers of a box plot (None: not given).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BoxStats {
    pub lower_whisker: Option<f64>,
    pub lower_quartile: Option<f64>,
    pub median: Option<f64>,
    pub average: Option<f64>,
    pub upper_quartile: Option<f64>,
    pub upper_whisker: Option<f64>,
    /// The plot's geometry keys, read in its survey: `draw position` (an expression of
    /// `\plotnumofactualtype`), `box extend`, `whisker extend`, `draw relative anchor`,
    /// `draw direction=y`.
    pub position: String,
    pub box_extend: f64,
    pub whisker_extend: f64,
    pub anchor: f64,
    pub vertical: bool,
    /// `variable width`: the sample size and `variable width min target`; the factor of the
    /// box's extend, set once all box plots of the axis are known.
    pub variable: bool,
    pub sample_size: Option<f64>,
    pub min_target: f64,
    pub factor: std::cell::Cell<f64>,
}

/// `boxplot/variable width`: each box's extend between `variable width min target` and 1 times
/// `box extend`, by the square root of its sample size among the axis' box plots
/// (`\pgfplotsplothandlerboxplotvisualization@variable@width`).
pub fn variable_widths(plots: &[&BoxStats]) {
    let keys: Vec<f64> = plots.iter().filter(|s| s.variable).filter_map(|s| s.sample_size.map(f64::sqrt)).collect();
    if keys.is_empty() {
        return;
    }
    let (lo, hi) = (keys.iter().copied().fold(f64::INFINITY, f64::min), keys.iter().copied().fold(f64::NEG_INFINITY, f64::max));
    for s in plots.iter().filter(|s| s.variable) {
        let Some(n) = s.sample_size else { continue };
        let t = ((n.sqrt() - lo) / (hi - lo)).clamp(0.0, 1.0);
        let t = if t.is_finite() { t } else { 1.0 };
        s.factor.set(s.min_target + t * (1.0 - s.min_target));
    }
}

thread_local! {
    /// The box plot being drawn, for `boxplot cs` and its friends: its geometry (logical).
    static CURRENT: std::cell::Cell<Option<Geometry>> = const { std::cell::Cell::new(None) };
}

/// A new axis: no box plot drawn yet (none of an earlier axis or figure).
pub fn reset() {
    CURRENT.with(|c| c.set(None));
}

/// `boxplot cs:v,b`, `boxplot box cs:v,b`, `boxplot whisker cs:v,b`: logical (x, y) of the box
/// plot being drawn.
pub fn cs(system: &str, value: f64, b: f64) -> Option<[f64; 2]> {
    let g = CURRENT.with(|c| c.get())?;
    let off = match system {
        "boxplot box" => (b - g.anchor) * g.box_extend,
        "boxplot whisker" => (b - g.anchor) * g.whisker_extend,
        _ => b,
    };
    Some(place(g.vertical, value, g.position + off))
}

/// A value of `boxplot/<name>`: None for empty or `auto`.
fn given(e: &mut Engine, name: &str) -> Option<f64> {
    let v = keys::value(e, &format!("/pgfplots/boxplot/{name}"));
    if v.is_empty() || v == "auto" {
        return None;
    }
    e.number(&v).ok().filter(|x| x.is_finite())
}

/// The percentile `p` of sorted `x` by `estimator`
/// (`\pgfplotsplothandler@boxplot@percentile@estimator@…`). `x` is not empty: a box plot without
/// values computes none.
pub fn percentile(x: &[f64], p: f64, estimator: &str) -> f64 {
    let n = x.len();
    // 1-based, clamped
    let get = |i: i64| x[(i - 1).clamp(0, n as i64 - 1) as usize];
    let lookup = |h: f64| {
        let low = h.floor();
        let (a, b) = (get(low as i64), get(low as i64 + 1));
        a + (h - low) * (b - a)
    };
    let nf = n as f64;
    match estimator {
        "R1" | "SAS3" | "Maple1" => get((p * nf).ceil() as i64),
        "R2" | "SAS5" | "Maple2" => {
            let h = p * nf + 0.5;
            0.5 * (get((h - 0.5).ceil() as i64) + get((h + 0.5).floor() as i64))
        }
        "R3" | "SAS2" => get((p * nf).round() as i64),
        "R4" | "SAS1" | "SciPy0-1" | "Maple3" => lookup(p * nf),
        "R5" | "SciPy12-12" | "Maple4" => lookup(p * nf + 0.5),
        "R6" | "SAS4" | "SciPy0-0" | "Maple5" => lookup(p * (nf + 1.0)),
        "R7" | "Excel" | "SciPy1-1" | "Maple6" => lookup((nf - 1.0) * p + 1.0),
        "R8" | "SciPy13-13" | "Maple7" => lookup((nf + 1.0 / 3.0) * p + 1.0 / 3.0),
        "R9" | "SciPy38-38" | "Maple8" => lookup((nf + 0.25) * p + 3.0 / 8.0),
        "legacy*" => {
            // 0-based indices
            let h = p * (nf - 1.0);
            let low = h.floor();
            let g = |i: i64| x[i.clamp(0, n as i64 - 1) as usize];
            if h > low {
                0.5 * (g(low as i64) + g(low as i64 + 1))
            } else {
                g(low as i64)
            }
        }
        _ => {
            // legacy
            let h = p * nf;
            let low = h.floor();
            if h > low {
                0.5 * (get(low as i64) + get(low as i64 + 1))
            } else {
                get(low as i64)
            }
        }
    }
}

/// The survey of a box plot: its numbers, and the outliers as its points.
pub fn survey(e: &mut Engine, plot: &mut Plot, prepared: bool) {
    let box_extend = keys::number(e, "/pgfplots/boxplot/box extend").unwrap_or(0.8);
    let mut stats = BoxStats {
        // unexpanded: `\plotnumofactualtype` is known when the plot is drawn
        position: crate::keys::value(&e.tex, "/pgfplots/boxplot/draw position").map(|v| crate::tex::tokens_to_string(&v)).unwrap_or_default(),
        box_extend,
        whisker_extend: keys::number(e, "/pgfplots/boxplot/whisker extend").unwrap_or(box_extend * 0.8),
        anchor: keys::number(e, "/pgfplots/boxplot/draw relative anchor").unwrap_or(0.5),
        vertical: keys::value(e, "/pgfplots/boxplot/draw direction") == "y",
        variable: keys::flag(e, "/pgfplots/boxplot/variable width"),
        sample_size: given(e, "sample size"),
        min_target: keys::number(e, "/pgfplots/boxplot/variable width min target").unwrap_or(0.2),
        factor: std::cell::Cell::new(1.0),
        lower_whisker: given(e, "lower whisker"),
        lower_quartile: given(e, "lower quartile"),
        median: given(e, "median"),
        average: given(e, "average"),
        upper_quartile: given(e, "upper quartile"),
        upper_whisker: given(e, "upper whisker"),
    };
    let data = keys::value(e, "/pgfplots/boxplot/data");
    let value = |c: &Coord| match data.as_str() {
        "x" => c.x,
        "z" => c.z,
        _ => c.y,
    };
    let mut values: Vec<f64> = plot.coords.iter().map(value).filter(|v| v.is_finite()).collect();
    if !prepared {
        values.sort_by(f64::total_cmp);
        let estimator = keys::value(e, "/pgfplots/boxplot/estimator");
        let off = |e: &mut Engine, k: &str, d: f64| keys::number(e, &format!("/pgfplots/boxplot/{k}")).unwrap_or(d);
        let (lo_off, hi_off) = (off(e, "lower quartile off", 0.25), off(e, "upper quartile off", 0.75));
        let range = off(e, "whisker range", 1.5);
        if !values.is_empty() {
            let lq = *stats.lower_quartile.get_or_insert_with(|| percentile(&values, lo_off, &estimator));
            let uq = *stats.upper_quartile.get_or_insert_with(|| percentile(&values, hi_off, &estimator));
            stats.median.get_or_insert_with(|| percentile(&values, 0.5, &estimator));
            // the average only when asked for (`average=auto`; its default is empty)
            if keys::value(e, "/pgfplots/boxplot/average") == "auto" {
                stats.average = Some(values.iter().sum::<f64>() / values.len() as f64);
            }
            let width = (uq - lq) * range;
            let (lw_limit, uw_limit) = (lq - width, uq + width);
            let mut outliers = Vec::new();
            let (mut lw, mut uw) = (None, None);
            for &v in &values {
                if v < lw_limit || v > uw_limit {
                    outliers.push(v);
                    continue;
                }
                if lw.is_none() {
                    lw = Some(v);
                }
                uw = Some(uw.map_or(v, |u: f64| u.max(v)));
            }
            if stats.lower_whisker.is_none() {
                stats.lower_whisker = lw;
            }
            if stats.upper_whisker.is_none() {
                stats.upper_whisker = uw;
            }
            if stats.sample_size.is_none() {
                stats.sample_size = Some(values.len() as f64);
            }
            values = outliers;
        }
    }
    // the outliers (or the points of a prepared box plot), placed in setup
    plot.coords = values.iter().map(|v| Coord::new(*v, 0.0, 0.0)).collect();
    plot.three_d = false;
    plot.scanlines.clear();
    plot.boxplot = Some(Box::new(stats));
}

/// `boxplot/draw position`, for the plot `index` of its kind.
pub fn draw_position(e: &mut Engine, expr: &str, index: usize) -> f64 {
    e.begin_group();
    e.tex.set_meaning(&crate::tex::Token::cs("plotnumofactualtype"), super::super::macro_from(&index.to_string()), false);
    let v = e.number(if expr.is_empty() { "1+\\plotnumofactualtype" } else { expr }).unwrap_or(1.0 + index as f64);
    e.end_group();
    v
}

/// (value, position) to logical coordinates, by `draw direction`.
fn place(vertical: bool, value: f64, pos: f64) -> [f64; 2] {
    if vertical {
        [pos, value]
    } else {
        [value, pos]
    }
}

/// The geometry keys of the box plot being drawn.
#[derive(Debug, Clone, Copy)]
struct Geometry {
    vertical: bool,
    position: f64,
    box_extend: f64,
    whisker_extend: f64,
    anchor: f64,
}

fn geometry(e: &mut Engine, s: &BoxStats, index: usize) -> Geometry {
    let position = draw_position(e, &s.position, index);
    let k = s.factor.get();
    Geometry { vertical: s.vertical, position, box_extend: s.box_extend * k, whisker_extend: s.whisker_extend * k, anchor: s.anchor }
}

/// The logical points a box plot covers (for the axis limits), the outliers placed at its position.
pub fn extent(e: &mut Engine, p: &Plot, index: usize, v: &mut [[f64; 3]]) -> Vec<[f64; 2]> {
    let Some(stats) = &p.boxplot else { return Vec::new() };
    // the limits see the box as wide as without `variable width`
    let mut g = geometry(e, stats, index);
    (g.box_extend, g.whisker_extend) = (stats.box_extend, stats.whisker_extend);
    for c in v.iter_mut() {
        let [a, b] = place(g.vertical, c[0], g.position);
        c[0] = a;
        c[1] = b;
    }
    let mut out = Vec::new();
    for (value, ext) in [
        (stats.lower_whisker, g.whisker_extend),
        (stats.upper_whisker, g.whisker_extend),
        (stats.lower_quartile, g.box_extend),
        (stats.upper_quartile, g.box_extend),
        (stats.median, g.box_extend),
        (stats.average, g.box_extend),
    ] {
        if let Some(x) = value {
            for b in [0.0, 1.0] {
                out.push(place(g.vertical, x, g.position + (b - g.anchor) * ext));
            }
        }
    }
    out
}

/// Draws the box, the whiskers, the median and the average (`\pgfplotsplothandlervisstart@boxplot@prepared@draw`).
pub fn draw(e: &mut Engine, p: &Plot, f: &Frame, index: usize) -> Result<(), Error> {
    let Some(stats) = p.boxplot.as_deref() else { return Ok(()) };
    let g = geometry(e, stats, index);
    // `\boxplotvalue{…}` and the box plot coordinate systems, for what follows the plot
    let num = |v: Option<f64>| v.map(super::super::pgf_number).unwrap_or_default();
    for (k, v) in [
        ("lower whisker", stats.lower_whisker),
        ("lower quartile", stats.lower_quartile),
        ("median", stats.median),
        ("average", stats.average),
        ("upper quartile", stats.upper_quartile),
        ("upper whisker", stats.upper_whisker),
    ] {
        keys::set(e, &format!("/pgfplots/boxplot/{k}"), &num(v));
    }
    keys::set(e, "/pgfplots/boxplot/draw position", &super::super::pgf_number(g.position));
    CURRENT.with(|c| c.set(Some(g)));
    let pt = |value: f64, b: f64| -> Point {
        let [x, y] = place(g.vertical, value, g.position + b);
        f.at([x, y, f.min[2]])
    };
    let boxb = |b: f64| (b - g.anchor) * g.box_extend;
    let whiskerb = |b: f64| (b - g.anchor) * g.whisker_extend;
    let (mut lq, mut uq) = (stats.lower_quartile, stats.upper_quartile);
    let has_box = lq.is_some() && uq.is_some();
    if !has_box {
        lq = stats.upper_whisker;
        uq = stats.lower_whisker;
    }
    for (whisker, quartile) in [(stats.lower_whisker, lq), (stats.upper_whisker, uq)] {
        let (Some(w), Some(q)) = (whisker, quartile) else { continue };
        let segs = vec![Segment::MoveTo(pt(q, 0.0)), Segment::LineTo(pt(w, 0.0)), Segment::MoveTo(pt(w, whiskerb(0.0))), Segment::LineTo(pt(w, whiskerb(1.0)))];
        e.draw_with("/pgfplots/boxplot/every whisker/.try", segs)?;
    }
    if let (true, Some(a), Some(b)) = (has_box, lq, uq) {
        let (p0, p1) = (pt(a, boxb(0.0)), pt(b, boxb(1.0)));
        let segs = vec![Segment::MoveTo(p0), Segment::LineTo(Point::new(p1.x, p0.y)), Segment::LineTo(p1), Segment::LineTo(Point::new(p0.x, p1.y)), Segment::Close];
        e.draw_with("/pgfplots/boxplot/every box/.try", segs)?;
    }
    if let Some(m) = stats.median {
        e.draw_with("/pgfplots/boxplot/every median/.try", vec![Segment::MoveTo(pt(m, boxb(0.0))), Segment::LineTo(pt(m, boxb(1.0)))])?;
    }
    if let Some(avg) = stats.average {
        let at = pt(avg, boxb(0.5));
        e.begin_group();
        let r = (|| {
            e.st.key_path = "/tikz".into();
            e.process_keys(super::toks("/pgfplots/boxplot/every average/.try"), Some("/tikz"))?;
            if let Some(name) = e.st.plot_mark.clone() {
                let size = e.st.mark_size;
                super::visual::draw_mark_set(e, &name, size, &crate::scene::Transform::default(), std::iter::once(at))?;
            }
            Ok(())
        })();
        e.end_group();
        r?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::percentile;

    #[test]
    fn estimators() {
        let x = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        // legacy: p·N whole: that element; otherwise the mean of two
        assert_eq!(percentile(&x, 0.5, "legacy"), 4.0);
        assert_eq!(percentile(&x, 0.3, "legacy"), 2.5);
        // R7 (Excel): (N-1)p+1 interpolated
        assert!((percentile(&x, 0.25, "R7") - 2.75).abs() < 1e-12);
        assert_eq!(percentile(&x, 0.5, "R7"), 4.5);
    }
}
