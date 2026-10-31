//! Exact size boundaries and calendar-aware date constraints.
use chrono::{DateTime, Local, NaiveDate, TimeZone};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub(super) struct Interval {
    lower: Option<(i128, bool)>,
    upper: Option<(i128, bool)>,
}
impl Interval {
    // Render the captured instants, never reparse dates under a later timezone.
    pub(super) fn date_bounds(&self) -> String {
        [(">", ">=", self.lower), ("<", "<=", self.upper)]
            .into_iter()
            .filter_map(|(open, closed, bound)| {
                bound.map(|(value, inclusive)| {
                    let timestamp = i64::try_from(value)
                        .ok()
                        .map(DateTime::from_timestamp_nanos)
                        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
                        .unwrap_or_else(|| "out-of-range date".into());
                    format!("{} {timestamp}", if inclusive { closed } else { open })
                })
            })
            .collect::<Vec<_>>()
            .join(" and ")
    }

    pub(super) fn contains(&self, value: i128) -> bool {
        self.lower
            .is_none_or(|(bound, inclusive)| value > bound || (inclusive && value == bound))
            && self
                .upper
                .is_none_or(|(bound, inclusive)| value < bound || (inclusive && value == bound))
    }
}

pub(super) const BYTE_SCALE: i128 = 1_000_000_000;

pub(super) fn size(text: &str) -> Result<Interval, String> {
    let (op, value) = comparison(text);
    if let Some((left, right)) = split_range(value, false) {
        if !op.is_empty() {
            return Err("A range cannot also have a comparison operator".into());
        }
        let (left, right) = (quantity(left)?, quantity(right)?);
        if left > right {
            return Err("Size range is reversed".into());
        }
        return Ok(Interval {
            lower: Some((left, true)),
            upper: Some((right, true)),
        });
    }
    Ok(compare(op, quantity(value)?, None))
}

fn quantity(text: &str) -> Result<i128, String> {
    let split = text
        .find(|c: char| c.is_ascii_alphabetic())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let multiplier: i128 = match unit.to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "kb" => 1_000,
        "mb" => 1_000_000,
        "gb" => 1_000_000_000,
        "tb" => 1_000_000_000_000,
        "kib" => 1 << 10,
        "mib" => 1 << 20,
        "gib" => 1 << 30,
        "tib" => 1 << 40,
        _ => return Err(format!("Unknown size unit: {unit}")),
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > 9
        || (number.contains('.') && fraction.is_empty())
    {
        return Err("Expected a nonnegative size with at most nine decimal places".into());
    }
    let whole: i128 = whole.parse().map_err(|_| "Size overflow")?;
    let fraction_value: i128 = if fraction.is_empty() {
        0
    } else {
        fraction.parse().map_err(|_| "Invalid size")?
    };
    let fixed = whole
        .checked_mul(BYTE_SCALE)
        .and_then(|v| v.checked_add(fraction_value * 10_i128.pow(9 - fraction.len() as u32)))
        .and_then(|v| v.checked_mul(multiplier))
        .ok_or("Size overflow")?;
    if fixed > i128::from(u64::MAX) * BYTE_SCALE {
        return Err("Size overflow".into());
    }
    Ok(fixed)
}

fn comparison(text: &str) -> (&str, &str) {
    for op in [">=", "<=", ">", "<", "="] {
        if let Some(value) = text.strip_prefix(op) {
            return (op, value);
        }
    }
    ("", text)
}
fn split_range(text: &str, date: bool) -> Option<(&str, &str)> {
    for delimiter in if date {
        ["...", "..", " - "]
    } else {
        ["...", "..", "-"]
    } {
        if let Some((left, right)) = text.split_once(delimiter) {
            return Some((left.trim(), right.trim()));
        }
    }
    None
}
fn compare(op: &str, value: i128, end: Option<i128>) -> Interval {
    match op {
        ">" => Interval {
            lower: Some((end.unwrap_or(value), end.is_some())),
            upper: None,
        },
        ">=" => Interval {
            lower: Some((value, true)),
            upper: None,
        },
        "<" => Interval {
            lower: None,
            upper: Some((value, false)),
        },
        "<=" => Interval {
            lower: None,
            upper: Some((end.unwrap_or(value), end.is_none())),
        },
        _ => Interval {
            lower: Some((value, true)),
            upper: Some((end.unwrap_or(value), end.is_none())),
        },
    }
}

pub(super) fn timestamp(time: SystemTime) -> i128 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(value) => value.as_nanos() as i128,
        Err(value) => -(value.duration().as_nanos() as i128),
    }
}
fn instant(text: &str) -> Result<(i128, Option<i128>), String> {
    if let Ok(date) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        let next = date.succ_opt().ok_or("Date overflow")?;
        let midnight = |day: NaiveDate| {
            Local
                .from_local_datetime(&day.and_hms_opt(0, 0, 0).unwrap())
                .earliest()
                .and_then(|time| time.timestamp_nanos_opt())
                .map(i128::from)
                .ok_or("Local midnight is unavailable or date is out of range")
        };
        return Ok((midnight(date)?, Some(midnight(next)?)));
    }
    let time = DateTime::parse_from_rfc3339(text)
        .map_err(|_| "Use YYYY-MM-DD or an RFC 3339 timestamp with an explicit offset")?;
    Ok((
        i128::from(time.timestamp_nanos_opt().ok_or("Date out of range")?),
        None,
    ))
}
pub(super) fn date(text: &str) -> Result<Interval, String> {
    let (op, value) = comparison(text);
    if let Some((left, right)) = split_range(value, true) {
        if !op.is_empty() {
            return Err("A date range cannot also have a comparison operator".into());
        }
        let ((start, start_end), (last, last_end)) = (instant(left)?, instant(right)?);
        if start_end.is_some() != last_end.is_some() {
            return Err("Do not mix calendar dates and timestamps in a range".into());
        }
        if start > last {
            return Err("Date range is reversed".into());
        }
        return Ok(Interval {
            lower: Some((start, true)),
            upper: Some((last_end.unwrap_or(last), last_end.is_none())),
        });
    }
    let (start, end) = instant(value)?;
    Ok(compare(op, start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn size_aliases_and_fractional_boundaries_are_exact() {
        for expression in ["10.5KB..2MB", "10.5KB...2MB", "10.5KB-2MB"] {
            let interval = size(expression).unwrap();
            assert!(!interval.contains(10_499 * BYTE_SCALE));
            assert!(interval.contains(10_500 * BYTE_SCALE));
            assert!(interval.contains(2_000_000 * BYTE_SCALE));
            assert!(!interval.contains(2_000_001 * BYTE_SCALE));
        }
        assert!(!size("<1.5B").unwrap().contains(2 * BYTE_SCALE));
        assert!(size(">1.5B").unwrap().contains(2 * BYTE_SCALE));
        assert!(size("=1KiB").unwrap().contains(1024 * BYTE_SCALE));
        for bad in [
            "-1B",
            "2KB-1KB",
            "1B....2B",
            "1B-",
            "1B-2B-3B",
            "18446744073709551616B",
        ] {
            assert!(size(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn local_calendar_days_follow_daylight_saving_boundaries() {
        const CHILD: &str = "LITTLE_CLIP_SEARCH_DST_TEST";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "file_manager::search::range::tests::local_calendar_days_follow_daylight_saving_boundaries",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("TZ", "America/New_York")
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "stdout: {}\nstderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"));
            return;
        }
        for (day, hours, lower, upper) in [
            (
                "2026-03-08",
                23,
                "2026-03-08T05:00:00Z",
                "2026-03-09T04:00:00Z",
            ),
            (
                "2026-11-01",
                25,
                "2026-11-01T04:00:00Z",
                "2026-11-02T05:00:00Z",
            ),
        ] {
            let (start, Some(end)) = instant(day).unwrap() else {
                panic!("Expected a whole local calendar day");
            };
            assert_eq!(start, instant(lower).unwrap().0);
            assert_eq!(end, instant(upper).unwrap().0);
            assert_eq!(end - start, hours * 3_600 * 1_000_000_000);
            assert_eq!(
                date(day).unwrap().date_bounds(),
                format!(">= {lower} and < {upper}")
            );
            for expression in [day.to_owned(), format!("{day}..{day}"), format!("={day}")] {
                let interval = date(&expression).unwrap();
                assert!(!interval.contains(start - 1));
                assert!(interval.contains(start));
                assert!(interval.contains(end - 1));
                assert!(!interval.contains(end));
            }
            assert!(!date(&format!("<{day}")).unwrap().contains(start));
            assert!(date(&format!(">={day}")).unwrap().contains(start));
            assert!(date(&format!("<={day}")).unwrap().contains(end - 1));
            assert!(!date(&format!("<={day}")).unwrap().contains(end));
            assert!(!date(&format!(">{day}")).unwrap().contains(end - 1));
            assert!(date(&format!(">{day}")).unwrap().contains(end));
        }
    }

    #[test]
    fn date_ranges_preserve_iso_hyphens_and_cover_complete_days() {
        let (start, Some(end)) = instant("2026-01-01").unwrap() else {
            panic!()
        };
        for expression in [
            "2026-01-01..2026-01-01",
            "2026-01-01...2026-01-01",
            "2026-01-01 - 2026-01-01",
        ] {
            let range = date(expression).unwrap();
            assert!(range.contains(start));
            assert!(range.contains(end - 1));
            assert!(!range.contains(end));
        }
        assert!(date("2026-01-01-2026-02-01").is_err());
        assert!(date("2026-01-01..2026-02-01T00:00:00Z").is_err());
        assert!(date(">2026-01-01").unwrap().contains(end));
        assert!(!date(">2026-01-01").unwrap().contains(end - 1));
    }
}
