//! Typed session answers. Neither questions nor answers enter the search database.
use super::query::AnswerType;
use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
mod context;
mod document;
pub(super) mod extraction;
pub(super) use document::{answer_prepared, read_document};
pub(super) mod visual;

#[derive(Clone, Debug, Serialize)]
pub(super) struct Source {
    pub id: usize,
    pub text: String,
    pub location: String,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Evidence {
    pub source: usize,
    pub quote: String,
}
#[derive(Clone, Debug)]
pub(super) enum State {
    Value(Value, String),
    NotApplicable,
    NotFound,
    Unknown,
    Ambiguous,
    Unsupported,
    Unavailable,
    Failed,
    Excluded,
}
#[derive(Clone, Debug)]
pub(super) struct Answer {
    pub applicability: Applicability,
    pub state: State,
    pub reason: String,
    pub evidence: Vec<(String, String)>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Applicability {
    Applicable,
    NotApplicable,
    Undetermined,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    applicability: String,
    state: String,
    value: Option<String>,
    unit: Option<String>,
    evidence: Vec<Evidence>,
    reason: String,
    #[serde(default)]
    basis: Option<String>,
}
impl Answer {
    pub fn status(state: State, reason: impl Into<String>) -> Self {
        let applicability = match state {
            State::Value(..) | State::NotFound | State::Ambiguous => Applicability::Applicable,
            State::NotApplicable => Applicability::NotApplicable,
            _ => Applicability::Undetermined,
        };
        Self {
            applicability,
            state,
            reason: reason.into(),
            evidence: Vec::new(),
        }
    }
    pub fn decode(
        reply: serde_json::Value,
        kind: AnswerType,
        sources: &[Source],
        complete: bool,
    ) -> Result<Self, String> {
        let wire: Wire = serde_json::from_value(reply).map_err(|_| "Invalid structured answer")?;
        let state = match (wire.applicability.as_str(), wire.state.as_str()) {
            ("applicable", "value") => {
                let raw = wire.value.as_deref().ok_or("Missing typed answer")?;
                let value = Value::parse(raw, wire.unit.as_deref(), kind)?;
                let display = wire
                    .unit
                    .as_ref()
                    .map_or_else(|| raw.to_owned(), |unit| format!("{raw} {unit}"));
                State::Value(value, display)
            }
            ("not_applicable", "not_applicable") => State::NotApplicable,
            ("applicable", "not_found") => State::NotFound,
            ("applicable" | "undetermined", "unknown") => State::Unknown,
            ("applicable", "ambiguous") => State::Ambiguous,
            _ => return Err("Contradictory applicability/answer states".into()),
        };
        if !matches!(state, State::Value(..)) && (wire.value.is_some() || wire.unit.is_some()) {
            return Err("A missing answer cannot carry a value".into());
        }
        let evidence = evidence(&wire.evidence, sources)?;
        if matches!(state, State::Value(..) | State::NotApplicable) && evidence.is_empty() {
            return Err("An answer requires source evidence".into());
        }
        if let Some(basis) = wire.basis.as_deref() {
            if !matches!(basis, "direct" | "global" | "unknown") {
                return Err("Invalid evidence sufficiency basis".into());
            }
            if basis != "direct" || matches!(state, State::NotApplicable | State::NotFound) {
                return Ok(Self::status(
                    State::Unknown,
                    "Selected quotations do not establish a complete answer.",
                ));
            }
        }
        let applicability = match wire.applicability.as_str() {
            "applicable" => Applicability::Applicable,
            "not_applicable" if complete => Applicability::NotApplicable,
            _ => Applicability::Undetermined,
        };
        if !complete {
            return Ok(Self {
                applicability,
                state: State::Unknown,
                reason: format!("Partial source coverage. {}", wire.reason),
                evidence,
            });
        }
        Ok(Self {
            applicability,
            state,
            reason: wire.reason,
            evidence,
        })
    }
    pub fn label(&self) -> &str {
        match &self.state {
            State::Value(_, value) => value,
            State::NotApplicable => "-",
            State::NotFound => "Not found",
            State::Unknown => "Unknown / partial",
            State::Ambiguous => "Ambiguous",
            State::Unsupported => "Unsupported",
            State::Unavailable => "Unavailable",
            State::Failed => "Failed",
            State::Excluded => "Excluded",
        }
    }
    pub fn compare(left: Option<&Self>, right: Option<&Self>, descending: bool) -> Ordering {
        match (left.and_then(Self::value), right.and_then(Self::value)) {
            (Some(a), Some(b)) => a.compare(b, descending),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    }
    fn value(&self) -> Option<&Value> {
        match &self.state {
            State::Value(value, _) => Some(value),
            _ => None,
        }
    }
}
fn evidence(items: &[Evidence], sources: &[Source]) -> Result<Vec<(String, String)>, String> {
    if items.len() > 4 {
        return Err("Too much answer evidence".into());
    }
    items
        .iter()
        .map(|item| {
            let source = sources
                .iter()
                .find(|source| source.id == item.source)
                .ok_or("Unknown evidence source")?;
            if item.quote.trim().is_empty()
                || item.quote.len() > 4096
                || !source.text.contains(&item.quote)
            {
                return Err("Answer evidence is not present in the current source".into());
            }
            Ok((source.location.clone(), item.quote.clone()))
        })
        .collect()
}

#[derive(Clone, Debug)]
pub(super) enum Value {
    Text(String),
    Number(Decimal, Option<String>),
    Date(u8, i128),
    Boolean(bool),
}
impl Value {
    fn parse(raw: &str, unit: Option<&str>, kind: AnswerType) -> Result<Self, String> {
        if raw.len() > 4096 || unit.is_some_and(|unit| unit.is_empty() || unit.len() > 64) {
            return Err("Answer value exceeds its budget".into());
        }
        if kind != AnswerType::Number && unit.is_some() {
            return Err("Unexpected answer unit".into());
        }
        match kind {
            AnswerType::Text => Ok(Self::Text(raw.to_lowercase())),
            AnswerType::Number => Ok(Self::Number(Decimal::parse(raw)?, unit.map(str::to_owned))),
            AnswerType::Boolean => match raw {
                "true" => Ok(Self::Boolean(true)),
                "false" => Ok(Self::Boolean(false)),
                _ => Err("Invalid boolean answer".into()),
            },
            AnswerType::Date => date(raw).map(|(group, key)| Self::Date(group, key)),
        }
    }
    fn compare(&self, other: &Self, descending: bool) -> Ordering {
        let directed = |order: Ordering| if descending { order.reverse() } else { order };
        match (self, other) {
            (Self::Text(a), Self::Text(b)) => {
                directed(crate::file_manager::sorting::natural_cmp(a, b))
            }
            (Self::Number(a, unit_a), Self::Number(b, unit_b)) => {
                unit_a.cmp(unit_b).then_with(|| directed(a.compare(b)))
            }
            (Self::Date(group_a, a), Self::Date(group_b, b)) => {
                group_a.cmp(group_b).then_with(|| directed(a.cmp(b)))
            }
            (Self::Boolean(a), Self::Boolean(b)) => directed(a.cmp(b)),
            _ => Ordering::Equal,
        }
    }
}
fn date(raw: &str) -> Result<(u8, i128), String> {
    let calendar = match raw.len() {
        4 => Some((0, format!("{raw}-01-01"))),
        7 => Some((1, format!("{raw}-01"))),
        10 => Some((2, raw.into())),
        _ => None,
    };
    if let Some((precision, text)) = calendar {
        let date =
            NaiveDate::parse_from_str(&text, "%Y-%m-%d").map_err(|_| "Invalid answer date")?;
        if date.year() < 1 || date.format("%Y-%m-%d").to_string() != text {
            return Err("Noncanonical answer date".into());
        }
        return Ok((precision, i128::from(date.num_days_from_ce())));
    }
    let instant = chrono::DateTime::parse_from_rfc3339(raw)
        .map_err(|_| "Expected an explicit date precision or offset timestamp")?;
    Ok((
        3,
        i128::from(instant.timestamp()) * 1_000_000_000
            + i128::from(instant.timestamp_subsec_nanos()),
    ))
}

/// Canonical decimal digits, without a floating-point conversion or precision cap.
#[derive(Clone, Debug)]
pub(super) struct Decimal {
    negative: bool,
    whole: String,
    fraction: String,
}
impl Decimal {
    fn parse(raw: &str) -> Result<Self, String> {
        let (negative, digits) = raw.strip_prefix('-').map_or((false, raw), |s| (true, s));
        let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
        if whole.is_empty()
            || !whole.bytes().all(|b| b.is_ascii_digit())
            || !fraction.bytes().all(|b| b.is_ascii_digit())
            || (digits.contains('.') && fraction.is_empty())
        {
            return Err("Expected an exact plain decimal answer".into());
        }
        let whole = whole.trim_start_matches('0');
        let fraction = fraction.trim_end_matches('0');
        Ok(Self {
            negative: negative && (!whole.is_empty() || !fraction.is_empty()),
            whole: if whole.is_empty() {
                "0".into()
            } else {
                whole.into()
            },
            fraction: fraction.into(),
        })
    }
    fn compare(&self, other: &Self) -> Ordering {
        let absolute = self
            .whole
            .len()
            .cmp(&other.whole.len())
            .then(self.whole.cmp(&other.whole))
            .then_with(|| {
                let length = self.fraction.len().max(other.fraction.len());
                self.fraction
                    .bytes()
                    .chain(std::iter::repeat(b'0'))
                    .take(length)
                    .cmp(
                        other
                            .fraction
                            .bytes()
                            .chain(std::iter::repeat(b'0'))
                            .take(length),
                    )
            });
        other.negative.cmp(&self.negative).then_with(|| {
            if self.negative {
                absolute.reverse()
            } else {
                absolute
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn source() -> Vec<Source> {
        vec![Source {
            id: 0,
            text: "No physical experiment was performed. The stated amount is USD 0.".into(),
            location: "page 1".into(),
        }]
    }
    fn wire(value: Option<&str>, state: &str, applicability: &str) -> serde_json::Value {
        json!({"applicability":applicability,"state":state,"value":value,"unit":null,"reason":"From the source", "evidence":[{"source":0,"quote":"No physical experiment was performed."}]})
    }
    #[test]
    fn false_zero_nonapplicability_and_unknown_remain_distinct() {
        let sources = source();
        let false_value = Answer::decode(
            wire(Some("false"), "value", "applicable"),
            AnswerType::Boolean,
            &sources,
            true,
        )
        .unwrap();
        assert_eq!(false_value.label(), "false");
        assert_eq!(false_value.applicability, Applicability::Applicable);
        let zero = Answer::decode(
            wire(Some("0"), "value", "applicable"),
            AnswerType::Number,
            &sources,
            true,
        )
        .unwrap();
        assert_eq!(zero.label(), "0");
        let absent = Answer::decode(
            wire(None, "not_applicable", "not_applicable"),
            AnswerType::Boolean,
            &sources,
            true,
        )
        .unwrap();
        assert_eq!(absent.label(), "-");
        assert_eq!(absent.applicability, Applicability::NotApplicable);
        assert!(matches!(
            Answer::decode(
                wire(None, "not_applicable", "not_applicable"),
                AnswerType::Boolean,
                &sources,
                false
            )
            .unwrap()
            .state,
            State::Unknown
        ));
        assert!(
            Answer::decode(
                wire(Some("false"), "not_applicable", "not_applicable"),
                AnswerType::Boolean,
                &sources,
                true
            )
            .is_err()
        );
        for descending in [false, true] {
            assert_eq!(
                Answer::compare(Some(&false_value), Some(&absent), descending),
                Ordering::Less
            );
        }
    }
    #[test]
    fn exact_decimals_sort_without_floating_point_rounding_or_digit_limits() {
        for (a, b) in [
            ("9007199254740992.01", "9007199254740992.02"),
            ("-100", "-2"),
            ("-0.01", "0"),
            ("0.009", "0.01"),
            ("9", "10"),
        ] {
            assert_eq!(
                Decimal::parse(a)
                    .unwrap()
                    .compare(&Decimal::parse(b).unwrap()),
                Ordering::Less,
                "{a} vs {b}"
            );
        }
        assert_eq!(
            Decimal::parse("-000.000")
                .unwrap()
                .compare(&Decimal::parse("0").unwrap()),
            Ordering::Equal
        );
        for bad in ["1,000", "1e3", "NaN", ".5", "1.", "--1"] {
            assert!(Decimal::parse(bad).is_err());
        }
        let a = Decimal::parse(&"9".repeat(1000)).unwrap();
        let b = Decimal::parse(&format!("1{}", "0".repeat(1000))).unwrap();
        assert_eq!(a.compare(&b), Ordering::Less);
    }
    #[test]
    fn currencies_and_date_precision_groups_stay_separate_in_both_directions() {
        let usd = Value::parse("1", Some("USD"), AnswerType::Number).unwrap();
        let eur = Value::parse("1000", Some("EUR"), AnswerType::Number).unwrap();
        assert_eq!(usd.compare(&eur, false), usd.compare(&eur, true));
        assert!(date("2027-02-29").is_err());
        assert!(date("0000").is_err());
        assert!(date("2027-01-01T10:00:00").is_err());
        assert_eq!(
            date("2027-01-01T10:00:00+02:00").unwrap(),
            date("2027-01-01T08:00:00Z").unwrap()
        );
        assert_ne!(date("2027").unwrap().0, date("2027-01-01").unwrap().0);
    }
    #[test]
    fn fabricated_evidence_and_invalid_types_cannot_be_accepted() {
        let sources = source();
        assert!(
            Answer::decode(
                wire(Some("yes"), "value", "applicable"),
                AnswerType::Boolean,
                &sources,
                true
            )
            .is_err()
        );
        let mut reply = wire(Some("true"), "value", "applicable");
        reply["evidence"][0]["quote"] = json!("A physical experiment was performed.");
        assert!(Answer::decode(reply, AnswerType::Boolean, &sources, true).is_err());
        let a = Value::parse("item2", None, AnswerType::Text).unwrap();
        let b = Value::parse("item10", None, AnswerType::Text).unwrap();
        assert_eq!(a.compare(&b, false), Ordering::Less);
    }
}
