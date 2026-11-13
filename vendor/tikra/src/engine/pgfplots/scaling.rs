//! The scaling of a 3d axis with computed unit vectors, as `pgfplots.scaling.code.tex` does it:
//! the plot box of unit size (the view's directions, times `plot box ratio`) is brought to the
//! target width and height either by stretching x and y on paper independently (`scale mode=
//! stretch to fill`, the default) or by one common scale for every unit vector (`scale mode=scale
//! uniformly`, implied by `unit vector ratio`), the axis limits then enlarged so that the box
//! keeps (about) its size.

use crate::scene::Point;

/// How the limits follow a rescaled unit vector (`unit rescale keep size`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepSize {
    False,
    True,
    UnlessLimitsDeclared,
}

impl KeepSize {
    pub fn parse(s: &str) -> KeepSize {
        match s.trim() {
            "false" => KeepSize::False,
            "unless limits declared" => KeepSize::UnlessLimitsDeclared,
            _ => KeepSize::True,
        }
    }
}

/// `scale uniformly strategy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Auto,
    UnitsOnly,
    ChangeVertical,
    ChangeHorizontal,
}

impl Strategy {
    pub fn parse(s: &str) -> Strategy {
        match s.trim() {
            "units only" => Strategy::UnitsOnly,
            "change vertical limits" => Strategy::ChangeVertical,
            "change horizontal limits" => Strategy::ChangeHorizontal,
            _ => Strategy::Auto,
        }
    }
}

/// What the scaling of a 3d axis needs.
pub struct Input<'a> {
    /// The unit vectors: of the plot box of unit size if computed, else as given; signs for
    /// `dir=reverse` included.
    pub e: [Point; 3],
    /// The target width and height of the plot box.
    pub width: f64,
    pub height: f64,
    /// Whether the unit vectors are computed (from the view) rather than given.
    pub unit_box: bool,
    pub three_d: bool,
    pub strategy: Strategy,
    /// `unit vector ratio`, empty if none.
    pub ratio: &'a str,
    /// `unit vector ratio axis`, empty to let it be chosen.
    pub ratio_axis: &'a str,
    pub keep: KeepSize,
    /// Which limits were computed from the data.
    pub auto_min: [bool; 3],
    pub auto_max: [bool; 3],
}

/// The unit vectors (per data unit) for `scale mode=scale uniformly`, and the limits, enlarged
/// as the scaling asks.
pub fn uniform(inp: &Input, min: &mut [f64; 3], max: &mut [f64; 3]) -> [Point; 3] {
    let zero = inp.e.map(|v| v.x == 0.0 && v.y == 0.0);
    let range = [0, 1, 2].map(|a| max[a] - min[a]);
    // the plot box: of unit size with computed unit vectors, else that of the limits
    let e = [0, 1, 2].map(|a| if inp.unit_box { inp.e[a] } else { inp.e[a].scale(range[a]) });
    // the inverse scale of each unit vector, to squeeze [min, max] into that box
    let mut inv = if inp.unit_box { range } else { [1.0; 3] };
    let mut limit = [1.0f64; 3];
    let (w, h) = box_size(&e);
    // one scale for all: the largest inverse scale wins, the others get longer limits
    let ext = (0..3).filter(|&a| !zero[a]).map(|a| inv[a]).fold(-16300.0, f64::max);
    for a in 0..3 {
        if !zero[a] && inp.strategy != Strategy::UnitsOnly {
            limit[a] = ext / inv[a];
        }
        inv[a] = ext;
    }
    let (s, r) = uniform_scale(inp, &e, w, h, limit);
    for (l, r) in limit.iter_mut().zip(r) {
        *l *= r;
    }
    unit_ratio(inp, &zero, &mut inv, &mut limit);
    minimize_limits(&zero, &mut inv, &mut limit);
    let ev = [0, 1, 2].map(|a| inp.e[a].scale(s / inv[a]));
    for (a, &l) in limit.iter().enumerate() {
        rescale_limits(inp, a, l, min, max);
    }
    ev
}

/// The size on paper of the box spanned by the three vectors.
fn box_size(e: &[Point; 3]) -> (f64, f64) {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for i in 0..8 {
        let mut p = Point::default();
        for (k, v) in e.iter().enumerate() {
            if (i >> k) & 1 == 1 {
                p = p.plus(*v);
            }
        }
        x0 = x0.min(p.x);
        y0 = y0.min(p.y);
        x1 = x1.max(p.x);
        y1 = y1.max(p.y);
    }
    (x1 - x0, y1 - y0)
}

/// `scale mode=scale uniformly`: the common scale and the factors by which the limits grow so that
/// the box fills width × height (`\pgfplots@get@scale@horiz@and@vert`); with the `auto` strategy,
/// the one that enlarges the limits least, none shrinking them.
fn uniform_scale(inp: &Input, e: &[Point; 3], w: f64, h: f64, limit: [f64; 3]) -> (f64, [f64; 3]) {
    let try_one = |s: Strategy| -> (f64, [f64; 3]) {
        let units_only = || {
            let s = if inp.width < inp.height { inp.width / w } else { inp.height / h };
            (s, [1.0; 3])
        };
        // the height of the box along one axis
        let unit_height = |a: usize| e[a].y.abs();
        match s {
            Strategy::Auto | Strategy::UnitsOnly => units_only(),
            Strategy::ChangeVertical => {
                // the width fits; the limits of the vertical axis give the height
                let s = inp.width / w;
                let a = if e[2].y == 0.0 {
                    if e[1].x == 0.0 {
                        1
                    } else {
                        return units_only();
                    }
                } else if e[2].x == 0.0 {
                    2
                } else {
                    return units_only();
                };
                let m = unit_height(a);
                let mut r = [1.0; 3];
                r[a] = (inp.height - s * (h - m)) / (s * m);
                (s, r)
            }
            Strategy::ChangeHorizontal => {
                if e[2].y == 0.0 {
                    if e[1].x == 0.0 && e[0].y == 0.0 {
                        let s = inp.height / h;
                        (s, [inp.width / w / s, 1.0, 1.0])
                    } else {
                        units_only()
                    }
                } else if e[2].x == 0.0 {
                    // x and y limits grow alike, z keeps its own: a fix point iteration
                    let m = unit_height(2);
                    let ww = inp.width / w;
                    let (mut s, mut r) = (1.0, 1.0);
                    for _ in 0..9 {
                        s = inp.height / (r * (h - m) + m);
                        r = ww / s;
                    }
                    (s, [r, r, 1.0])
                } else {
                    units_only()
                }
            }
        }
    };
    if inp.strategy != Strategy::Auto {
        return try_one(inp.strategy);
    }
    let cost = |r: &[f64; 3]| -> f64 {
        let total = [0, 1, 2].map(|a| limit[a] * r[a]);
        if total.iter().any(|&t| t < 1.0) {
            f64::INFINITY
        } else {
            total.iter().sum()
        }
    };
    let horizontal = try_one(Strategy::ChangeHorizontal);
    let vertical = try_one(Strategy::ChangeVertical);
    let (ch, cv) = (cost(&horizontal.1), cost(&vertical.1));
    let best = if cv < ch { (vertical, cv) } else { (horizontal, ch) };
    if best.1.is_infinite() {
        try_one(Strategy::UnitsOnly)
    } else {
        best.0
    }
}

/// `unit vector ratio` (`\pgfplots@apply@unit@ratio`): the unit vectors other than a reference get
/// the asked lengths relative to it, their limits growing as they shrink.
fn unit_ratio(inp: &Input, zero: &[bool; 3], inv: &mut [f64; 3], limit: &mut [f64; 3]) {
    if inp.ratio.trim().is_empty() {
        return;
    }
    let mut r: Vec<f64> = inp.ratio.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    r.extend([1.0, 1.0, 1.0]);
    let given = [r[0], r[1], r[2]];
    // equal ratios are what `scale uniformly` did already
    if given[0] == given[1] && (given[0] == given[2] || !inp.three_d) {
        return;
    }
    // the ratios relative to the reference axis
    let relative = |reference: usize| -> [f64; 3] {
        let mut k = 1.0 / given[reference];
        if reference != 2 {
            k *= given[2];
        }
        given.map(|g| g * k)
    };
    // the scale of axis a for ratio q relative to the reference
    let scale = |a: usize, reference: usize, q: f64, inv: &[f64; 3]| -> f64 {
        if zero[a] || zero[reference] {
            16001.0
        } else {
            inv[a] / inv[reference] * q
        }
    };
    let reference = match inp.ratio_axis.trim() {
        "x" => 0,
        "y" => 1,
        "z" => 2,
        _ => {
            // the axis that keeps its length: the others only shrink, and as little as can be
            let one = 1.002;
            let mut best: Option<(usize, f64)> = None;
            for reference in 0..3 {
                let q = relative(reference);
                let others: Vec<usize> = (0..3).filter(|&a| a != reference).collect();
                let sa = scale(others[0], reference, q[others[0]], inv);
                let sb = scale(others[1], reference, q[others[1]], inv);
                if one - sa >= 0.0 && one - sb >= 0.0 {
                    let m = (one - sa).max(one - sb);
                    if best.is_none_or(|(_, b)| m < b) {
                        best = Some((reference, m));
                    }
                }
            }
            best.map(|b| b.0).unwrap_or(if zero[2] { 1 } else { 2 })
        }
    };
    let q = relative(reference);
    let before = *inv;
    for a in 0..3 {
        if zero[a] || (a == reference && (q[a] - 1.0).abs() <= 0.0002) {
            continue;
        }
        let s = scale(a, reference, q[a], &before);
        inv[a] /= s;
        limit[a] /= s;
    }
}

/// `\pgfplots@scaling@minimize@limitrescale`: limits that all grow are grown less, the unit
/// vectors made longer by as much.
fn minimize_limits(zero: &[bool; 3], inv: &mut [f64; 3], limit: &mut [f64; 3]) {
    // (no smallest: no unit vector, or no limit that is a number)
    let smallest = (0..3).filter(|&a| !zero[a]).map(|a| limit[a]).fold(f64::INFINITY, f64::min);
    if (0..3).any(|a| !zero[a] && limit[a] < 1.002) || !smallest.is_finite() {
        return;
    }
    for a in 0..3 {
        if !zero[a] || a < 2 {
            inv[a] /= smallest;
            limit[a] /= smallest;
        }
    }
}

/// `\pgfplots@apply@unit@vector@rescale@keep@size`: the range of axis a grows by `factor`, on the
/// sides whose limits were computed.
fn rescale_limits(inp: &Input, a: usize, factor: f64, min: &mut [f64; 3], max: &mut [f64; 3]) {
    if factor == 1.0 || inp.keep == KeepSize::False {
        return;
    }
    let d = (factor - 1.0) * (max[a] - min[a]);
    let (lo, hi) = match (inp.auto_min[a], inp.auto_max[a]) {
        (true, true) => (d / 2.0, d / 2.0),
        (true, false) => (d, 0.0),
        (false, true) => (0.0, d),
        (false, false) => {
            if inp.keep == KeepSize::True {
                (d / 2.0, d / 2.0)
            } else {
                (0.0, 0.0)
            }
        }
    };
    min[a] -= lo;
    max[a] += hi;
}
