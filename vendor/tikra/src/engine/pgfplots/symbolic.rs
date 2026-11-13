//! `symbolic x coords={a,b,c}` (and y, z): names that stand for 0, 1, 2… wherever a coordinate of
//! that axis is read (points, table cells, `xtick`, `xmin`), and are printed back as its tick
//! labels.

use super::keys;
use super::Engine;

/// The names of axis `c`'s symbolic coordinates (none: not symbolic).
pub fn names(e: &Engine, c: char) -> Vec<String> {
    let Some(v) = crate::keys::value(&e.tex, &format!("/pgfplots/symbolic {c} coords")) else { return Vec::new() };
    list(&crate::tex::tokens_to_string(&v))
}

/// The number a name stands for on axis `c` (or a date, with `date coordinates in`).
pub fn index(e: &mut Engine, c: char, text: &str) -> Option<f64> {
    if super::dateplot::is_date(e, c) {
        return super::dateplot::value(e, text);
    }
    let names = names(e, c);
    // `[normalized]1.5`: the number itself (`symbolic coords/magic prefix`)
    if !names.is_empty() {
        if let Some(n) = crate::keys::strip_braces(text.trim()).trim().strip_prefix("[normalized]") {
            return super::table::parse_number(n.trim());
        }
    }
    position(&names, text)
}

/// The number a name stands for as a histogram's data (`hist/symbolic coords`).
pub fn hist_index(e: &Engine, text: &str) -> Option<f64> {
    let v = crate::keys::value(&e.tex, "/pgfplots/hist/@symbolic coords")?;
    position(&list(&crate::tex::tokens_to_string(&v)), text)
}

fn position(names: &[String], text: &str) -> Option<f64> {
    let t = crate::keys::strip_braces(text.trim()).trim();
    names.iter().position(|n| n == t).map(|i| i as f64)
}

fn list(text: &str) -> Vec<String> {
    crate::keys::split_top_level(text).iter().map(|s| crate::keys::strip_braces(s.trim()).trim().to_string()).filter(|s| !s.is_empty()).collect()
}

/// A key's number (`xmin=tool1` on a symbolic axis is its index).
pub fn number(e: &mut Engine, c: char, key: &str) -> Option<f64> {
    let v = keys::value(e, key);
    if !v.is_empty() {
        if let Some(i) = index(e, c, &v) {
            return Some(i);
        }
    }
    keys::number(e, key)
}

/// The name a number stands for on a symbolic axis: the nearest one, the first or the last
/// beyond them (`/pgfplots/x coord inv trafo`), for tick labels and `/data point/x`.
pub fn label(e: &Engine, c: char, t: f64) -> Option<String> {
    let names = names(e, c);
    if names.is_empty() {
        return None;
    }
    let k = if t.is_finite() { t.round().clamp(0.0, (names.len() - 1) as f64) } else { 0.0 };
    Some(names[k as usize].clone())
}
