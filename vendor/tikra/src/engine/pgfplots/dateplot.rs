//! The dateplot library: `date coordinates in=x` reads `2009-08-18 09:30` as days (and the
//! time as a fraction of a day) since the first date the axis reads (`date ZERO`), and gives a
//! tick label code `\year`, `\month`, `\day`, `\hour` and `\minute` of the tick (`\tick` is
//! `2009-08-18`).

use crate::tex::Token;

use super::super::{macro_from, Engine};

/// The macro that keeps the julian day of `date ZERO`, global as in pgfplots.
const ZERO: &str = "pgfplots@tikra@date@zero";

/// Whether axis `c` has `date coordinates in=c`.
pub fn is_date(e: &Engine, c: char) -> bool {
    crate::keys::value(&e.tex, "/pgfplots/@date coordinates in").is_some_and(|v| crate::tex::tokens_to_string(&v).contains(c))
}

/// At the start of an axis, `date ZERO` is the first date it reads again.
pub fn reset(e: &mut Engine) {
    e.tex.set_meaning(&Token::cs(ZERO), macro_from(""), true);
}

/// The number for `2009-08-18` or `2009-08-18 09:30` on a date axis.
pub fn value(e: &mut Engine, text: &str) -> Option<f64> {
    let t = crate::keys::strip_braces(text.trim()).trim();
    let (date, time) = match t.split_once(char::is_whitespace) {
        Some((d, rest)) if rest.contains(':') => (d, Some(rest.trim())),
        _ => (t, None),
    };
    let julian = crate::engine::calendar::parse_date(date, None)?;
    let zero = match e.tex.expand_to_string(vec![Token::cs(ZERO)]).ok().and_then(|s| s.trim().parse::<i64>().ok()) {
        Some(z) => z,
        None => {
            let z = match crate::keys::value(&e.tex, "/pgfplots/date ZERO").map(|v| crate::tex::tokens_to_string(&v)) {
                Some(s) if !s.trim().is_empty() => crate::engine::calendar::parse_date(s.trim(), None).unwrap_or(julian),
                _ => julian,
            };
            e.tex.set_meaning(&Token::cs(ZERO), macro_from(&z.to_string()), true);
            z
        }
    };
    let days = julian - zero;
    let Some(time) = time else { return Some(days as f64) };
    // the fraction of the day as TeX divides dimensions: hours/24 + minutes/1440, at most 1
    let mut parts = time.split(':').map(|p| p.trim().parse::<i64>().unwrap_or(0));
    let (h, m) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    let sp = ((h * 65536) / 24 + (m * 65536) / 1440).clamp(0, 65536);
    if sp >= 65536 {
        return Some((days + 1) as f64);
    }
    // the digits of the fraction follow the day's: `-1.5` for half a day after the day before
    let frac = super::super::pgf_number(sp as f64 / 65536.0);
    let digits = frac.split_once('.').map_or("0", |(_, d)| d);
    format!("{days}.{digits}").parse().ok()
}

/// For a tick label at `t`: `\tick` is the date and `\year`… `\minute` its parts
/// (`/pgfplots/x coord inv trafo`).
pub fn define_tick(e: &mut Engine, t: f64) {
    let text = super::super::pgf_number(t);
    let (int, frac) = text.split_once('.').unwrap_or((&text, "0"));
    let days: i64 = int.parse().unwrap_or(0);
    // the fraction to hours and minutes, as TeX does it in dimensions
    let f: f64 = format!("0.{frac}").parse().unwrap_or(0.0);
    let mut x = (f * 65536.0).round() as i64 * 24;
    let mut hour = x / 65536;
    x -= hour * 65536;
    x *= 60;
    let mut minute = x / 65536;
    if x - minute * 65536 > 32768 {
        minute += 1;
        if minute == 60 {
            hour += 1;
            minute = 0;
        }
    }
    let zero = e.tex.expand_to_string(vec![Token::cs(ZERO)]).ok().and_then(|s| s.trim().parse::<i64>().ok()).unwrap_or(0);
    let (year, month, day) = crate::engine::calendar::date(days + zero);
    let (month, day) = (format!("{month:02}"), format!("{day:02}"));
    let set = |e: &mut Engine, name: &str, v: &str| e.tex.set_meaning(&Token::cs(name), macro_from(v), false);
    set(e, "year", &year.to_string());
    set(e, "month", &month);
    set(e, "day", &day);
    set(e, "Hour", &hour.to_string());
    set(e, "Minute", &minute.to_string());
    set(e, "hour", &format!("{hour:02}"));
    set(e, "minute", &format!("{minute:02}"));
    set(e, "Second", "0");
    set(e, "second", "00");
    set(e, "lowlevel", &text);
    set(e, "tick", &format!("{year}-{month}-{day}"));
}
