use std::{collections::BTreeSet, sync::LazyLock};

use super::super::{RawAnalysisKind, SpiceRawDocument};
use regex::Regex;

const MAX_CARDS: usize = 64;
const MAX_LINE: usize = 256;
const MAX_TOKEN: usize = 32;
const MAX_STEPS: f64 = 20_000.0;
const MAX_STOP: f64 = 60.0;

static NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^([+-]?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:e[+-]?[0-9]+)?)(meg|[tgkmunpf])?$")
        .expect("fixed SPICE numeric syntax")
});

pub(super) fn admit(bytes: &[u8]) -> Result<Analyses, String> {
    let lines = card_lines(bytes)?;
    let mut names = BTreeSet::new();
    let mut analyses = Analyses::default();
    let mut ended = false;
    for (index, original) in lines.enumerate() {
        let line = original.trim();
        if original.len() > MAX_LINE {
            return Err(format!("Line {} exceeds {MAX_LINE} bytes", index + 2));
        }
        if line.is_empty() || line.starts_with('*') {
            continue;
        }
        let line = line.to_ascii_uppercase();
        let mut tokens = line.split_ascii_whitespace();
        let card = tokens.next().unwrap_or_default();
        let fields: Vec<_> = tokens.collect();
        let admitted = !ended
            && match card {
                ".END" if fields.is_empty() => {
                    ended = true;
                    true
                }
                ".OP" | ".TRAN" => analyses.add(card, &fields),
                _ => element(card, &fields, &mut names),
            };
        if !admitted {
            return Err(format!(
                "Line {} is outside the supported self-contained simulation subset",
                index + 2
            ));
        }
    }
    if !ended || names.is_empty() || !(analyses.op || analyses.stop.is_some()) {
        return Err("The deck needs elements, .op or .tran, and a final .end".into());
    }
    Ok(analyses)
}

fn card_lines(bytes: &[u8]) -> Result<std::str::Lines<'_>, String> {
    if !bytes.iter().enumerate().all(|(index, byte)| match byte {
        b'\r' => bytes.get(index + 1) == Some(&b'\n'),
        b'\t' | b'\n' | b' '..=b'~' => true,
        _ => false,
    }) {
        return Err("Simulation requires ASCII text with LF or CRLF line endings".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "Simulation deck is not ASCII")?;
    let mut lines = text.lines();
    let title = lines.next().unwrap_or_default().trim();
    if title.len() > MAX_LINE
        || !title
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        || !title
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b" _+-,.\t".contains(&byte))
    {
        return Err("Use a plain title on the first line of the simulation deck".into());
    }
    Ok(lines)
}

#[derive(Default)]
pub(super) struct Analyses {
    op: bool,
    stop: Option<f64>,
}

impl Analyses {
    fn add(&mut self, card: &str, fields: &[&str]) -> bool {
        match (card, fields) {
            (".OP", []) if !self.op => {
                self.op = true;
                true
            }
            (".TRAN", [step, stop]) if self.stop.is_none() => {
                let (Some(step), Some(stop)) = (number(step), number(stop)) else {
                    return false;
                };
                let bounded =
                    (0.0 < step && step <= stop && stop <= MAX_STOP) && stop / step <= MAX_STEPS;
                self.stop = bounded.then_some(stop);
                bounded
            }
            _ => false,
        }
    }

    pub(super) fn verify(&self, raw: &SpiceRawDocument) -> Result<(), String> {
        let has_data = |kind| {
            raw.plots.iter().find(|plot| {
                plot.kind == kind && plot.traces.iter().any(|trace| !trace.samples().is_empty())
            })
        };
        if self.op && has_data(RawAnalysisKind::DcOperatingPoint).is_none() {
            return Err("Solver output is missing the requested operating point".into());
        }
        if let Some(stop) = self.stop {
            let reached_stop = has_data(RawAnalysisKind::Transient)
                .and_then(|plot| plot.x_range)
                .is_some_and(|[_, last]| (last - stop).abs() <= (stop * 1e-9).max(1e-15));
            if !reached_stop {
                return Err("Solver output did not reach the requested transient end time".into());
            }
        }
        Ok(())
    }
}

fn element(card: &str, fields: &[&str], names: &mut BTreeSet<String>) -> bool {
    let [node_a, node_b, value @ ..] = fields else {
        return false;
    };
    if !word(card)
        || !word(node_a)
        || !word(node_b)
        || node_a == node_b
        || names.len() >= MAX_CARDS
        || !names.insert(card.to_owned())
    {
        return false;
    }
    match card.as_bytes()[0] {
        b'R' | b'C' | b'L' => matches!(value, [value] if number(value).is_some_and(|n| n > 0.0)),
        b'I' | b'V' => source(value),
        _ => false,
    }
}

fn word(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= MAX_TOKEN
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn source(fields: &[&str]) -> bool {
    let fields = match fields {
        ["DC", value, rest @ ..] if number(value).is_some() => {
            if rest.is_empty() {
                return true;
            }
            rest
        }
        [value] if number(value).is_some() => return true,
        fields => fields,
    };
    let form = fields.join(" ");
    let Some(arguments) = form
        .strip_prefix("PULSE(")
        .and_then(|value| value.strip_suffix(')'))
    else {
        return false;
    };
    let numbers = arguments
        .split_ascii_whitespace()
        .map(number)
        .collect::<Option<Vec<_>>>();
    let Some([_, _, delay, rise, fall, width, period]) = numbers.as_deref() else {
        return false;
    };
    *delay >= 0.0 && *rise > 0.0 && *fall > 0.0 && *width > 0.0 && rise + fall + width <= *period
}

fn number(text: &str) -> Option<f64> {
    if text.len() > MAX_TOKEN {
        return None;
    }
    let captures = NUMBER.captures(text)?;
    let value: f64 = captures.get(1)?.as_str().parse().ok()?;
    let factor = match captures.get(2).map(|suffix| suffix.as_str()) {
        None => 1.0,
        Some("T") => 1e12,
        Some("G") => 1e9,
        Some("MEG") => 1e6,
        Some("K") => 1e3,
        Some("M") => 1e-3,
        Some("U") => 1e-6,
        Some("N") => 1e-9,
        Some("P") => 1e-12,
        Some("F") => 1e-15,
        _ => return None,
    };
    let quantity = value * factor;
    (quantity == 0.0 || (1e-15..=1e12).contains(&quantity.abs())).then_some(quantity)
}
