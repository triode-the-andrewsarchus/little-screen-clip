//! Positions along a plot (`\pgfplotspointplotattime`, pgfplotscoordprocessing.code.tex): `node
//! [pos=t]` after `\addplot` is at the fraction `t` of the plot's length (the polyline through its
//! points, in the data scale transformation, not across jumps), on the segment there, so `sloped`
//! follows it; `\pgfplotspointplotattime` in a node's text sets `/data point/x` and its friends
//! to that point.

use std::cell::RefCell;

use crate::tex::Token;
use crate::Error;

use super::Engine;

/// The plot whose nodes are being placed.
pub struct Timeline {
    /// Logical points (None at jumps).
    pub points: Vec<Option<[f64; 3]>>,
    /// The data scale per axis: lengths are measured in `10^E · x`.
    pub scale: [f64; 3],
    pub log: [bool; 3],
    /// The `pos` of the node being placed.
    pub time: f64,
}

thread_local! {
    static CURRENT: RefCell<Option<Timeline>> = const { RefCell::new(None) };
}

pub fn set(t: Option<Timeline>) {
    CURRENT.with(|c| *c.borrow_mut() = t);
}

pub fn set_time(time: f64) {
    CURRENT.with(|c| {
        if let Some(t) = c.borrow_mut().as_mut() {
            t.time = time;
        }
    });
}

/// A point along a plot: where it is, the ends of the segment it is on and how far along that
/// segment.
pub type AtTime = ([f64; 3], [f64; 3], [f64; 3], f64);

impl Timeline {
    fn len(&self, a: [f64; 3], b: [f64; 3]) -> f64 {
        (0..3).map(|k| ((b[k] - a[k]) * self.scale[k]).powi(2)).filter(|v| v.is_finite()).sum::<f64>().sqrt()
    }

    /// The point at fraction `t`, the segment it is on and the fraction of that segment
    /// (`\pgfplotspointplotattime@`: the first segment whose end is beyond the target length).
    pub fn at(&self, t: f64) -> Option<AtTime> {
        let pairs: Vec<([f64; 3], [f64; 3])> = self.points.windows(2).filter_map(|w| Some((w[0]?, w[1]?))).collect();
        let Some(&(first, second)) = pairs.first() else {
            let p = self.points.iter().flatten().next().copied()?;
            return Some((p, p, p, 0.0));
        };
        let total: f64 = pairs.iter().map(|(a, b)| self.len(*a, *b)).sum();
        let target = t * total;
        let mut done = 0.0;
        let mut last = (first, second, 0.0, self.len(first, second));
        for (a, b) in pairs {
            let l = self.len(a, b);
            if target < done + l {
                let f = if l > 0.0 { (target - done) / l } else { 0.0 };
                return Some((lerp(a, b, f), a, b, f));
            }
            last = (a, b, done, done + l);
            done += l;
        }
        let (a, b, from, to) = last;
        let f = if to > from { (target - from) / (to - from) } else { 1.0 };
        Some((lerp(a, b, f), a, b, f))
    }
}

fn lerp(a: [f64; 3], b: [f64; 3], f: f64) -> [f64; 3] {
    [0, 1, 2].map(|k| a[k] + f * (b[k] - a[k]))
}

/// `\pgfplotspointplotattime` or `\pgfplotspointplotattime{t}` at `tokens[i]` in a node's text:
/// sets `/data point/x`, `y`, `z` and returns where the text goes on.
pub fn run(e: &mut Engine, tokens: &[Token], i: usize) -> Result<usize, Error> {
    // (TeX skips the spaces after a control word: the argument, if any, is next)
    let j = i + 1;
    let (arg, end) = if j < tokens.len() && matches!(tokens[j], Token::Char(_, crate::tex::Cat::Begin)) {
        let mut depth = 0;
        let mut k = j;
        while k < tokens.len() {
            match tokens[k] {
                Token::Char(_, crate::tex::Cat::Begin) => depth += 1,
                Token::Char(_, crate::tex::Cat::End) => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            k += 1;
        }
        (Some(tokens[j + 1..k.min(tokens.len())].to_vec()), k + 1)
    } else {
        (None, i + 1)
    };
    let t = match arg {
        Some(a) => Some(e.number_of(a)?),
        None => None,
    };
    let point = CURRENT.with(|c| c.borrow().as_ref().and_then(|tl| tl.at(t.unwrap_or(tl.time)).map(|r| (r.0, tl.log))));
    match point {
        Some((p, log)) => {
            for (k, c) in ['x', 'y', 'z'].iter().enumerate() {
                let v = if log[k] { p[k].exp() } else { p[k] };
                let text = super::symbolic::label(e, *c, v).unwrap_or_else(|| super::super::pgf_number(v));
                super::keys::set(e, &format!("/data point/{c}"), &text);
            }
        }
        None => e.warn("\\pgfplotspointplotattime outside the nodes of a plot: ignored"),
    }
    Ok(end)
}
