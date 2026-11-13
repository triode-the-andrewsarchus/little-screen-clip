//! Tick positions, as `pgfplotsticks.code.tex` computes them
//! (`\pgfplots@assign@default@tick@foraxis` and its helpers).
//!
//! Linear axes: the number of ticks is `axis length / max space between ticks + 1` (at least `try
//! min ticks`), the distance `range / (n - 1)` is snapped to 1, 2 or 5 times a power of ten, and
//! the ticks are the multiples of it in the range. Logarithmic axes (in natural logarithms, as
//! pgfplots keeps them) tick at whole powers of ten, or every j-th one if the range is large.

use std::f64::consts::LN_10;

/// Settings of the tick algorithm for one axis.
#[derive(Debug, Clone, Copy)]
pub struct TickSettings {
    /// The axis length, in pt.
    pub length_pt: f64,
    /// `max space between ticks` (pt, initially 35).
    pub max_space: f64,
    /// `try min ticks` (4) or `try min ticks log` (3).
    pub try_min: usize,
    /// `xtick distance`, if given (in data units; for log axes a factor).
    pub distance: Option<f64>,
    /// The data range and the `width` (or `height`) option in pt: they choose the data scale
    /// transformation the distance is computed in (`\pgfplots@set@optimal@datatrafo@for@`).
    pub scale: Option<(f64, f64, f64)>,
}

/// `floor(log10 |v|) + 1`, pgf's float exponent plus one.
fn order(v: f64) -> i32 {
    v.abs().log10().floor() as i32 + 1
}

/// The distance of `n` ticks over [min, max] as TeX computes it: the limits in the data scale
/// transformation `T(x) = 10^E x - shift` as dimensions, `\divide` (truncating), back to data units.
fn tex_distance(min: f64, max: f64, n: usize, (dmin, dmax, size): (f64, f64, f64)) -> f64 {
    let f = data_scale(dmin, dmax, size);
    let shift = dmin * f;
    let sp = |v: f64| (v * 65536.0).round();
    let lo = sp(min * f - shift);
    let hi = sp(max * f - shift) + sp(0.001);
    let h_sp = ((hi - lo).abs() / (n.max(2) - 1) as f64).trunc();
    h_sp / 65536.0 / f
}

/// The factor `10^E` of the data scale transformation for a data range and a size in pt
/// (`\pgfplots@set@optimal@datatrafo@for@`).
pub fn data_scale(dmin: f64, dmax: f64, size: f64) -> f64 {
    let m = match (dmin == 0.0, dmax == 0.0) {
        (true, true) => 1,
        (true, false) => order(dmax),
        (false, true) => order(dmin),
        _ => order(dmin).max(order(dmax)),
    };
    let mut exp = order(size) - m;
    // avoid cancellation: while T(max) - T(min) is small, scale up
    for _ in 0..30 {
        let f = 10f64.powi(exp);
        let (tmax, diff) = ((dmax * f).abs(), (dmax - dmin) * f);
        if tmax < 1500.0 && (0.0001..100.0).contains(&diff) {
            exp += 1;
        } else {
            break;
        }
    }
    10f64.powi(exp)
}

/// Snaps a tick distance to 1, 2 or 5 times a power of ten
/// (`\pgfplots@assign@default@tick@foraxis@normalizetickdist`).
pub fn normalize_distance(h: f64) -> f64 {
    if !(h.is_finite() && h > 0.0) {
        return h;
    }
    let mut e = h.log10().floor();
    let mut m = h / 10f64.powf(e);
    // guard the mantissa against the floor of an inexact logarithm
    // (a mantissa just above 10, of a logarithm a little short, snaps to 10 below anyway)
    if m < 1.0 {
        m *= 10.0;
        e -= 1.0;
    }
    let snapped = if m < 2.0 {
        if m < 1.5 {
            1.0
        } else {
            2.0
        }
    } else if m < 4.9999 {
        if m < 3.5 {
            2.0
        } else {
            5.0
        }
    } else if m < 7.5 {
        5.0
    } else {
        e += 1.0;
        1.0
    };
    snapped * 10f64.powf(e)
}

/// The number of ticks wanted for an axis: one per `max space` pt, plus one, at least `try_min`.
fn desired(s: &TickSettings) -> usize {
    let n = if s.max_space > 0.0 { (s.length_pt / s.max_space).floor().max(0.0) as usize + 1 } else { 1 };
    n.max(s.try_min)
}

/// Linear ticks: the list of candidates (some may lie outside [min, max]: they are filtered when
/// drawn) and the distance between them.
pub fn linear(min: f64, max: f64, s: TickSettings) -> (Vec<f64>, f64) {
    let mut s = s;
    for _ in 0..15 {
        let (h, fixed) = match s.distance {
            Some(d) if d.is_finite() && d > 0.0 => (d.abs(), true),
            _ => {
                let n = desired(&s);
                match s.scale.filter(|(a, b, z)| a.is_finite() && b.is_finite() && *z > 0.0) {
                    Some(sc) => (tex_distance(min, max, n, sc), false),
                    None => ((max - min).abs() / (n.max(2) - 1) as f64, false),
                }
            }
        };
        let h = if fixed { h } else { normalize_distance(h) };
        if !(h.is_finite() && h > 0.0) {
            return (Vec::new(), 0.0);
        }
        // MIN = trunc(min / H) * H - H: one tick below, it is not drawn
        let first = (min / h).trunc() * h - h;
        let top = max + 1e-9 * (max - min).abs().max(h);
        if !fixed && top < first + 2.0 * h {
            // a single tick label would be useless: ask for more ticks
            s.try_min += 1;
            continue;
        }
        let mut ticks = Vec::new();
        let mut k = 0usize;
        loop {
            let v = first + k as f64 * h;
            if v > top + 0.5 * h || ticks.len() > 10_000 {
                break;
            }
            ticks.push(clean(v, h));
            k += 1;
        }
        return (ticks, h);
    }
    (Vec::new(), 0.0)
}

/// Removes the floating point noise of `k * h` (0.30000000000000004 → 0.3) relative to the step.
fn clean(v: f64, h: f64) -> f64 {
    if v.abs() < h * 1e-9 {
        return 0.0;
    }
    // (finite: v is at most some 10^4 steps of h, and a step is at least an ulp of v)
    let scale = 10f64.powi((-(h.abs().log10().floor()) as i32 + 12).clamp(-300, 300));
    (v * scale).round() / scale
}

/// A value as a TeX dimension, in sp.
fn to_sp(v: f64) -> i64 {
    let max = crate::tex::dimen::MAX_DIMEN as f64;
    (v * 65536.0).round().clamp(-max, max) as i64
}

/// `0.4343\dimen`: a length in natural logarithms in decades, as pgfplots' ticks compute it (1/ln 10
/// is 0.4343 in its fpu, `log from display log`).
fn by_one_over_ln10(sp: i64) -> i64 {
    crate::tex::dimen::xn_over_d(sp, crate::tex::dimen::round_decimals(&[4, 3, 4, 3]), 65536).0
}

/// Logarithmic ticks, in natural logarithms: (ticks, distance, uniform). `uniform` is true when
/// every power of ten gets a tick (then minor ticks are drawn).
pub fn logarithmic(min: f64, max: f64, s: TickSettings) -> (Vec<f64>, f64, bool) {
    let n = desired(&s);
    let range = (max - min).abs();
    let (h_ten, uniform) = match s.distance {
        Some(d) if d.is_finite() && d > 0.0 => (d.ln() / LN_10, false),
        _ => {
            // in TeX dimensions: (max + 0.001pt - min) / (n - 1), times 0.4343 (`log from display
            // log`), the floor of that in decades
            let range_sp = (to_sp(max) + 66 - to_sp(min)).abs();
            let raw = by_one_over_ln10(range_sp / (n.max(2) - 1) as i64) as f64 / 65536.0;
            let j = if raw < 2.0 { 1.0 } else { raw.floor() };
            (j, j == 1.0)
        }
    };
    let h = h_ten * LN_10;
    if !(h.is_finite() && h > 0.0) {
        return (Vec::new(), 0.0, false);
    }
    // MIN = I * log(10), I the truncated min / log(10), one less for negatives, in TeX
    // dimensions too: -25.32768 is -10.9997 decades there, so I is -11, not -12
    let m = by_one_over_ln10(to_sp(min));
    let mut i = (m / 65536) as f64;
    if m < 0 {
        i -= 1.0;
    }
    let mut first = i * LN_10;
    if !uniform && 2.0 * (min - first) - h < 0.0 {
        first += LN_10;
    }
    let top = max + 1e-9 + 0.5 * h;
    let mut ticks = Vec::new();
    let mut k = 0usize;
    // too few ticks: pgfplots retries with ticks at 10^(i*h) for a fractional h
    // only one label (the axis ends before the second tick: `MAX < MIN + 2H`)
    if max + 1e-9 < first + 2.0 * h && s.distance.is_none() {
        let mut n2 = n.saturating_sub(1).max(1);
        if n2 > 2 {
            n2 -= 1;
        }
        let hh = normalize_distance(range / LN_10 / n2 as f64);
        let mut ii = (min / LN_10 / hh).trunc();
        if min < 0.0 {
            ii -= 1.0;
        }
        let start = ii * hh * LN_10;
        let step = hh * LN_10;
        loop {
            let v = start + k as f64 * step;
            if v > max + 1e-9 + 0.5 * step || ticks.len() > 1000 {
                break;
            }
            ticks.push(v);
            k += 1;
        }
        return (ticks, step, false);
    }
    loop {
        let v = first + k as f64 * h;
        if v > top || ticks.len() > 10_000 {
            break;
        }
        ticks.push(v);
        k += 1;
    }
    (ticks, h, uniform)
}

/// The exponent `scaled ticks=true` divides the tick labels by: 0 when the limits' order of
/// magnitude is within [below, above] (`\pgfplots@init@scaled@tick@for`).
pub fn scale_exponent(min: f64, max: f64, above: i32, below: i32) -> i32 {
    let exp = |v: f64| if v == 0.0 || !v.is_finite() { None } else { Some(v.abs().log10().floor() as i32 + correction(v)) };
    let (a, b) = match (exp(min), exp(max)) {
        (None, None) => (0, 0),
        (None, Some(b)) => (b, b),
        (Some(a), None) => (a, a),
        (Some(a), Some(b)) => (a, b),
    };
    let e = a.max(b);
    if e > above || e < below {
        -e
    } else {
        0
    }
}

/// log10 can be off by one at exact powers of ten: compare with the decimal exponent.
fn correction(v: f64) -> i32 {
    let s = format!("{:e}", v.abs());
    let e: i32 = s.split_once('e').and_then(|(_, e)| e.parse().ok()).unwrap_or(0);
    e - (v.abs().log10().floor() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(length_pt: f64) -> TickSettings {
        TickSettings { length_pt, max_space: 35.0, try_min: 4, distance: None, scale: None }
    }

    fn visible(ticks: &[f64], min: f64, max: f64) -> Vec<f64> {
        ticks.iter().copied().filter(|t| *t >= min - 1e-9 && *t <= max + 1e-9).collect()
    }

    #[test]
    fn snapping() {
        assert_eq!(normalize_distance(2.4), 2.0);
        assert_eq!(normalize_distance(0.24), 0.2);
        assert_eq!(normalize_distance(1.6), 2.0);
        assert_eq!(normalize_distance(3.6), 5.0);
        assert_eq!(normalize_distance(8.0), 10.0);
        assert_eq!(normalize_distance(1.2), 1.0);
        assert_eq!(normalize_distance(600.0), 500.0);
    }

    #[test]
    fn default_axis_ticks() {
        // 240pt wide: 195pt of axis, 6 ticks wanted; x from -6 to 6 (domain -5:5 enlarged)
        let (t, h) = linear(-6.0, 6.0, settings(195.0));
        assert_eq!(h, 2.0);
        assert_eq!(visible(&t, -6.0, 6.0), vec![-6.0, -4.0, -2.0, 0.0, 2.0, 4.0, 6.0]);
        // 162pt high: 5 ticks; y from -2.5 to 27.5 (data 0 to 25, height=207pt): TeX's
        // distance is 7.500024 (the 0.001pt guard), which snaps to 10
        let (t, h) = linear(-2.5, 27.5, TickSettings { scale: Some((0.0, 25.0, 207.0)), ..settings(162.0) });
        assert_eq!(h, 10.0);
        assert_eq!(visible(&t, -2.5, 27.5), vec![0.0, 10.0, 20.0]);
        // a matrix plot's cells from -0.5 to 2.5 around data 0 to 2: 0.7500024 snaps to 1
        let (_, h) = linear(-0.5, 2.5, TickSettings { scale: Some((0.0, 2.0, 207.0)), ..settings(162.0) });
        assert_eq!(h, 1.0);
        let (t, _) = linear(0.0, 1.0, settings(195.0));
        assert_eq!(visible(&t, 0.0, 1.0), vec![0.0, 0.2, 0.4, 0.6, 0.8, 1.0]);
    }

    #[test]
    fn log_ticks_at_powers_of_ten() {
        let (t, _, uniform) = logarithmic(0.0, 4.0 * LN_10, TickSettings { try_min: 3, ..settings(162.0) });
        assert!(uniform);
        let decades: Vec<f64> = t.iter().map(|v| (v / LN_10 * 1e6).round() / 1e6).filter(|v| (0.0..=4.0).contains(v)).collect();
        assert_eq!(decades, vec![0.0, 1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn tick_scaling() {
        assert_eq!(scale_exponent(0.0, 10000.0, 3, -1), -4);
        assert_eq!(scale_exponent(0.0, 1000.0, 3, -1), 0);
        assert_eq!(scale_exponent(0.0, 0.01, 3, -1), 2);
        assert_eq!(scale_exponent(-5.0, 5.0, 3, -1), 0);
    }
}
