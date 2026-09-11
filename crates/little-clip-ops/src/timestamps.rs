use chrono::{DateTime, Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimePrecision {
    Day,
    Hour,
    Minute,
    Second,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimestampPreview {
    pub original: String,
    pub utc_iso: String,
    pub human_readable: String,
    pub relative: String,
    pub epoch_sec: i64,
    pub day_info: String,
    pub precision: TimePrecision,
}

impl TimestampPreview {
    pub fn summary_lines(&self) -> Vec<String> {
        vec![
            format!("📅 {}", self.human_readable),
            format!("🌐 UTC: {}", self.utc_iso),
            format!("⏱ Relative: {}", self.relative),
            format!("🔢 Epoch: {} ({})", self.epoch_sec, self.day_info),
        ]
    }
}

/// Formats relative time from now with given precision.
pub fn format_relative_time(target_dt: &DateTime<Utc>, precision: TimePrecision) -> String {
    format_relative_time_from(target_dt, &Utc::now(), precision)
}

/// Formats relative time from a reference `now` timestamp.
///
/// Rules:
/// - If > 90 days: `+/- YY:MM:DD`
/// - If <= 90 days: `+/- DD:HH:MM:SS` (never more accurate than unit provided):
///   - Day: `+/-{days}D`
///   - Hour: `+/-{hours}H` when days == 0 (e.g. -1H at 2:10 for 1PM, -2H at 2:31), else `+/-DD:HH`
///   - Minute: `+/-DD:HH:MM`
///   - Second: `+/-DD:HH:MM:SS`
pub fn format_relative_time_from(
    target_dt: &DateTime<Utc>,
    now_dt: &DateTime<Utc>,
    precision: TimePrecision,
) -> String {
    let diff_sec = target_dt.timestamp() - now_dt.timestamp();
    let sign = if diff_sec >= 0 { '+' } else { '-' };
    let abs_sec = diff_sec.abs();

    // Check if more than 90 days
    let days_raw = abs_sec / 86400;
    if days_raw > 90 {
        let years = days_raw / 365;
        let rem_days = days_raw % 365;
        let months = rem_days / 30;
        let days = rem_days % 30;
        return format!("{}{:02}:{:02}:{:02}", sign, years, months, days);
    }

    match precision {
        TimePrecision::Day => {
            // Round to nearest day (cutoff at 12h = 43200s)
            let rounded_days = (abs_sec + 43200) / 86400;
            format!("{}{}D", sign, rounded_days)
        }
        TimePrecision::Hour => {
            // Round to nearest hour (cutoff at 30m = 1800s)
            let total_hours = (abs_sec + 1800) / 3600;
            let days = total_hours / 24;
            let hours = total_hours % 24;
            if days == 0 {
                format!("{}{}H", sign, hours)
            } else {
                format!("{}{:02}:{:02}", sign, days, hours)
            }
        }
        TimePrecision::Minute => {
            // Round to nearest minute (cutoff at 30s)
            let total_mins = (abs_sec + 30) / 60;
            let days = total_mins / 1440;
            let rem_mins = total_mins % 1440;
            let hours = rem_mins / 60;
            let mins = rem_mins % 60;
            format!("{}{:02}:{:02}:{:02}", sign, days, hours, mins)
        }
        TimePrecision::Second => {
            let days = abs_sec / 86400;
            let rem_sec = abs_sec % 86400;
            let hours = rem_sec / 3600;
            let rem2 = rem_sec % 3600;
            let mins = rem2 / 60;
            let secs = rem2 % 60;
            format!("{}{:02}:{:02}:{:02}:{:02}", sign, days, hours, mins, secs)
        }
    }
}

fn local_from_naive(naive: &NaiveDateTime) -> Option<DateTime<Local>> {
    match Local.from_local_datetime(naive) {
        chrono::LocalResult::Single(dt) => Some(dt),
        chrono::LocalResult::Ambiguous(dt, _) => Some(dt),
        chrono::LocalResult::None => None,
    }
}

fn parse_month_name(s: &str) -> Option<u32> {
    match s.to_ascii_lowercase().as_str() {
        "january" | "jan" => Some(1),
        "february" | "feb" => Some(2),
        "march" | "mar" => Some(3),
        "april" | "apr" => Some(4),
        "may" => Some(5),
        "june" | "jun" => Some(6),
        "july" | "jul" => Some(7),
        "august" | "aug" => Some(8),
        "september" | "sep" | "sept" => Some(9),
        "october" | "oct" => Some(10),
        "november" | "nov" => Some(11),
        "december" | "dec" => Some(12),
        _ => None,
    }
}

fn parse_day_number(s: &str) -> Option<u32> {
    let lower = s.to_ascii_lowercase();
    let clean = if lower.ends_with("st") || lower.ends_with("nd") || lower.ends_with("rd") || lower.ends_with("th") {
        if lower.len() <= 2 {
            return None;
        }
        &lower[..lower.len() - 2]
    } else {
        &lower[..]
    };
    let d: u32 = clean.parse().ok()?;
    if (1..=31).contains(&d) {
        Some(d)
    } else {
        None
    }
}

fn parse_year(s: &str) -> Option<i32> {
    let clean = s.trim_matches(|c: char| !c.is_ascii_digit());
    let y: i32 = clean.parse().ok()?;
    if clean.len() == 2 && (0..100).contains(&y) {
        if y <= 69 {
            Some(2000 + y)
        } else {
            Some(1900 + y)
        }
    } else if clean.len() == 4 && (1900..=2100).contains(&y) {
        Some(y)
    } else {
        None
    }
}

/// Parses time strings such as "1:00 pm", "1pm", "1:00:30 PM", "13:00", "13:00:00".
fn parse_time_component(s: &str) -> Option<(u32, u32, u32, TimePrecision)> {
    let lower = s.to_ascii_lowercase().replace('.', "");
    let trimmed = lower.trim();

    let is_pm = trimmed.ends_with("pm");
    let is_am = trimmed.ends_with("am");

    if is_pm || is_am {
        let core = trimmed.trim_end_matches("pm").trim_end_matches("am").trim();
        let parts: Vec<&str> = core.split(':').collect();
        match parts.len() {
            1 => {
                let mut h: u32 = parts[0].trim().parse().ok()?;
                if !(1..=12).contains(&h) {
                    return None;
                }
                if is_pm && h < 12 {
                    h += 12;
                } else if is_am && h == 12 {
                    h = 0;
                }
                Some((h, 0, 0, TimePrecision::Hour))
            }
            2 => {
                let mut h: u32 = parts[0].trim().parse().ok()?;
                let m: u32 = parts[1].trim().parse().ok()?;
                if !(1..=12).contains(&h) || m >= 60 {
                    return None;
                }
                if is_pm && h < 12 {
                    h += 12;
                } else if is_am && h == 12 {
                    h = 0;
                }
                Some((h, m, 0, TimePrecision::Minute))
            }
            3 => {
                let mut h: u32 = parts[0].trim().parse().ok()?;
                let m: u32 = parts[1].trim().parse().ok()?;
                let sec: u32 = parts[2].trim().parse().ok()?;
                if !(1..=12).contains(&h) || m >= 60 || sec >= 60 {
                    return None;
                }
                if is_pm && h < 12 {
                    h += 12;
                } else if is_am && h == 12 {
                    h = 0;
                }
                Some((h, m, sec, TimePrecision::Second))
            }
            _ => None,
        }
    } else {
        let parts: Vec<&str> = trimmed.split(':').collect();
        match parts.len() {
            2 => {
                let h: u32 = parts[0].trim().parse().ok()?;
                let m: u32 = parts[1].trim().parse().ok()?;
                if h < 24 && m < 60 {
                    Some((h, m, 0, TimePrecision::Minute))
                } else {
                    None
                }
            }
            3 => {
                let h: u32 = parts[0].trim().parse().ok()?;
                let m: u32 = parts[1].trim().parse().ok()?;
                let sec: u32 = parts[2].trim().parse().ok()?;
                if h < 24 && m < 60 && sec < 60 {
                    Some((h, m, sec, TimePrecision::Second))
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

/// Parses numeric dates like "02/05/25", "02/05/2025", "2/5/25", "2025-02-05", with optional time.
fn parse_numeric_date(input: &str) -> Option<(DateTime<Utc>, TimePrecision)> {
    let parts: Vec<&str> = input.split_whitespace().collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }

    let date_str = parts[0];
    let time_str = if parts.len() > 1 {
        Some(parts[1..].join(" "))
    } else {
        None
    };

    let date_tokens: Vec<&str> = date_str
        .split(|c| c == '/' || c == '-' || c == '.')
        .filter(|s| !s.is_empty())
        .collect();

    if date_tokens.len() != 3 {
        return None;
    }

    let a: i32 = date_tokens[0].parse().ok()?;
    let b: u32 = date_tokens[1].parse().ok()?;

    let (year, month, day) = if a >= 1000 {
        // YYYY-MM-DD
        let c: u32 = date_tokens[2].parse().ok()?;
        (a, b, c)
    } else {
        // MM/DD/YYYY or MM/DD/YY
        let y = parse_year(date_tokens[2])?;
        if a > 12 && b <= 12 {
            // DD/MM/YYYY
            (y, b, a as u32)
        } else {
            // MM/DD/YYYY
            (y, a as u32, b)
        }
    };

    let naive_date = NaiveDate::from_ymd_opt(year, month, day)?;

    if let Some(ref t_str) = time_str {
        let (h, m, s, prec) = parse_time_component(t_str)?;
        let naive_time = NaiveTime::from_hms_opt(h, m, s)?;
        let naive_dt = naive_date.and_time(naive_time);
        let local_dt = local_from_naive(&naive_dt)?;
        Some((local_dt.with_timezone(&Utc), prec))
    } else {
        let naive_time = NaiveTime::from_hms_opt(0, 0, 0)?;
        let naive_dt = naive_date.and_time(naive_time);
        let local_dt = local_from_naive(&naive_dt)?;
        Some((local_dt.with_timezone(&Utc), TimePrecision::Day))
    }
}

/// Parses textual date expressions like "May 2nd, 2025", "May 2 2025", "2nd May 2025", with optional time.
fn parse_textual_date(input: &str) -> Option<(DateTime<Utc>, TimePrecision)> {
    let clean = input.replace(',', " ");
    let words: Vec<&str> = clean.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }

    let month_idx = words.iter().position(|w| parse_month_name(w).is_some())?;
    let month = parse_month_name(words[month_idx])?;

    let mut day_opt: Option<u32> = None;
    let mut year_opt: Option<i32> = None;
    let mut used_indices = vec![month_idx];

    // Check before month: e.g. "2nd May 2025"
    if month_idx > 0 {
        if let Some(d) = parse_day_number(words[month_idx - 1]) {
            day_opt = Some(d);
            used_indices.push(month_idx - 1);
        }
    }

    // Check after month: e.g. "May 2nd 2025"
    if day_opt.is_none() && month_idx + 1 < words.len() {
        let next_word = words[month_idx + 1];
        if next_word.eq_ignore_ascii_case("of") && month_idx + 2 < words.len() {
            used_indices.push(month_idx + 1);
            if let Some(d) = parse_day_number(words[month_idx + 2]) {
                day_opt = Some(d);
                used_indices.push(month_idx + 2);
            }
        } else if let Some(d) = parse_day_number(next_word) {
            day_opt = Some(d);
            used_indices.push(month_idx + 1);
        }
    }

    let day = day_opt?;

    // Look for year among remaining tokens
    for (idx, word) in words.iter().enumerate() {
        if used_indices.contains(&idx) {
            continue;
        }
        if word.eq_ignore_ascii_case("of") || word.eq_ignore_ascii_case("at") || word.eq_ignore_ascii_case("on") {
            used_indices.push(idx);
            continue;
        }
        if let Some(y) = parse_year(word) {
            year_opt = Some(y);
            used_indices.push(idx);
            break;
        }
    }

    let year = year_opt.unwrap_or_else(|| Local::now().year());
    let naive_date = NaiveDate::from_ymd_opt(year, month, day)?;

    // Remaining tokens may form time
    let mut remaining_words = Vec::new();
    for (idx, word) in words.iter().enumerate() {
        if !used_indices.contains(&idx) && !word.eq_ignore_ascii_case("at") && !word.eq_ignore_ascii_case("on") {
            remaining_words.push(*word);
        }
    }

    if !remaining_words.is_empty() {
        let time_str = remaining_words.join(" ");
        if let Some((h, m, s, prec)) = parse_time_component(&time_str) {
            let naive_time = NaiveTime::from_hms_opt(h, m, s)?;
            let naive_dt = naive_date.and_time(naive_time);
            let local_dt = local_from_naive(&naive_dt)?;
            return Some((local_dt.with_timezone(&Utc), prec));
        }
    }

    let naive_time = NaiveTime::from_hms_opt(0, 0, 0)?;
    let naive_dt = naive_date.and_time(naive_time);
    let local_dt = local_from_naive(&naive_dt)?;
    Some((local_dt.with_timezone(&Utc), TimePrecision::Day))
}

pub fn detect_timestamp(text: &str) -> Option<TimestampPreview> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.len() > 60 || trimmed.lines().count() > 1 {
        return None;
    }

    let mut dt_opt: Option<(DateTime<Utc>, TimePrecision)> = None;

    // 1. Pure digits: epoch timestamp
    if trimmed.chars().all(|c| c.is_ascii_digit()) {
        if let Ok(num) = trimmed.parse::<i64>() {
            match trimmed.len() {
                // 9-10 digits: Seconds (1973 - 2100)
                9..=10 => {
                    if num >= 100_000_000 && num <= 4_102_444_800 {
                        if let Some(dt) = Utc.timestamp_opt(num, 0).single() {
                            dt_opt = Some((dt, TimePrecision::Second));
                        }
                    }
                }
                // 12-13 digits: Milliseconds
                12..=13 => {
                    let sec = num / 1000;
                    let ms = (num % 1000) as u32;
                    if sec >= 100_000_000 && sec <= 4_102_444_800 {
                        if let Some(dt) = Utc.timestamp_opt(sec, ms * 1_000_000).single() {
                            dt_opt = Some((dt, TimePrecision::Second));
                        }
                    }
                }
                // 15-16 digits: Microseconds
                15..=16 => {
                    let sec = num / 1_000_000;
                    let us = (num % 1_000_000) as u32;
                    if sec >= 100_000_000 && sec <= 4_102_444_800 {
                        if let Some(dt) = Utc.timestamp_opt(sec, us * 1000).single() {
                            dt_opt = Some((dt, TimePrecision::Second));
                        }
                    }
                }
                // 18-19 digits: Nanoseconds
                18..=19 => {
                    let sec = num / 1_000_000_000;
                    let ns = (num % 1_000_000_000) as u32;
                    if sec >= 100_000_000 && sec <= 4_102_444_800 {
                        if let Some(dt) = Utc.timestamp_opt(sec, ns).single() {
                            dt_opt = Some((dt, TimePrecision::Second));
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // 2. ISO 8601 / RFC 3339 strings
    if dt_opt.is_none() {
        if let Ok(parsed) = DateTime::parse_from_rfc3339(trimmed) {
            dt_opt = Some((parsed.with_timezone(&Utc), TimePrecision::Second));
        } else if let Ok(parsed) = DateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S") {
            dt_opt = Some((parsed.with_timezone(&Utc), TimePrecision::Second));
        } else if let Ok(naive) = NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S") {
            dt_opt = Some((DateTime::from_naive_utc_and_offset(naive, Utc), TimePrecision::Second));
        } else if let Ok(naive) = NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S") {
            dt_opt = Some((DateTime::from_naive_utc_and_offset(naive, Utc), TimePrecision::Second));
        } else if let Ok(naive) = NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M") {
            dt_opt = Some((DateTime::from_naive_utc_and_offset(naive, Utc), TimePrecision::Minute));
        } else if let Ok(naive) = NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M") {
            dt_opt = Some((DateTime::from_naive_utc_and_offset(naive, Utc), TimePrecision::Minute));
        }
    }

    // 3. Time-only expressions: e.g. "1:00 pm", "1pm", "13:00" -> assume current local date
    if dt_opt.is_none() {
        if let Some((h, m, s, prec)) = parse_time_component(trimmed) {
            let now_local = Local::now();
            let date = now_local.date_naive();
            if let Some(naive_time) = NaiveTime::from_hms_opt(h, m, s) {
                let naive_dt = date.and_time(naive_time);
                if let Some(local_dt) = local_from_naive(&naive_dt) {
                    dt_opt = Some((local_dt.with_timezone(&Utc), prec));
                }
            }
        }
    }

    // 4. Numeric Slash / Dash / Dot Dates: e.g. "02/05/25", "02/05/2025", "2/5/25"
    if dt_opt.is_none() {
        if let Some((dt, prec)) = parse_numeric_date(trimmed) {
            dt_opt = Some((dt, prec));
        }
    }

    // 5. Textual Month Dates: e.g. "May 2nd, 2025", "May 2 2025", "2nd May 2025"
    if dt_opt.is_none() {
        if let Some((dt, prec)) = parse_textual_date(trimmed) {
            dt_opt = Some((dt, prec));
        }
    }

    let (dt, precision) = dt_opt?;
    let local_dt = dt.with_timezone(&Local);

    let utc_iso = dt.to_rfc3339();
    let human_readable = match precision {
        TimePrecision::Day => local_dt.format("%a, %b %d, %Y").to_string(),
        TimePrecision::Hour => local_dt.format("%a, %b %d, %Y %I %p %Z").to_string(),
        TimePrecision::Minute => local_dt.format("%a, %b %d, %Y %I:%M %p %Z").to_string(),
        TimePrecision::Second => local_dt.format("%a, %b %d, %Y %I:%M:%S %p %Z").to_string(),
    };
    let relative = format_relative_time(&dt, precision);
    let epoch_sec = dt.timestamp();
    let day_info = format!("Day {}, Wk {}", dt.format("%j"), dt.format("%W"));

    Some(TimestampPreview {
        original: trimmed.to_string(),
        utc_iso,
        human_readable,
        relative,
        epoch_sec,
        day_info,
        precision,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timestamp_epoch_sec() {
        let ts = detect_timestamp("1725990000").expect("1725990000");
        assert_eq!(ts.epoch_sec, 1725990000);
        assert!(ts.utc_iso.starts_with("2024-09-10"));
        assert_eq!(ts.precision, TimePrecision::Second);
    }

    #[test]
    fn test_timestamp_epoch_ms() {
        let ts = detect_timestamp("1725990000000").expect("1725990000000");
        assert_eq!(ts.epoch_sec, 1725990000);
        assert!(ts.utc_iso.starts_with("2024-09-10"));
        assert_eq!(ts.precision, TimePrecision::Second);
    }

    #[test]
    fn test_timestamp_iso8601() {
        let ts = detect_timestamp("2026-09-10T17:44:11Z").expect("ISO8601");
        assert_eq!(ts.utc_iso, "2026-09-10T17:44:11+00:00");
        assert!(ts.human_readable.contains("2026"));
        assert_eq!(ts.precision, TimePrecision::Second);
    }

    #[test]
    fn test_parse_slash_dates() {
        let ts1 = detect_timestamp("02/05/25").expect("02/05/25");
        assert_eq!(ts1.precision, TimePrecision::Day);
        assert!(ts1.human_readable.contains("Feb 05, 2025") || ts1.human_readable.contains("Feb 5, 2025"));

        let ts2 = detect_timestamp("02/05/2025").expect("02/05/2025");
        assert_eq!(ts2.precision, TimePrecision::Day);
        assert!(ts2.human_readable.contains("Feb 05, 2025") || ts2.human_readable.contains("Feb 5, 2025"));
    }

    #[test]
    fn test_parse_textual_dates() {
        let ts1 = detect_timestamp("May 2nd, 2025").expect("May 2nd, 2025");
        assert_eq!(ts1.precision, TimePrecision::Day);
        assert!(ts1.human_readable.contains("May 02, 2025") || ts1.human_readable.contains("May 2, 2025"));

        let ts2 = detect_timestamp("May 2 2025").expect("May 2 2025");
        assert_eq!(ts2.precision, TimePrecision::Day);
        assert!(ts2.human_readable.contains("May 02, 2025") || ts2.human_readable.contains("May 2, 2025"));

        let ts3 = detect_timestamp("2nd May 2025").expect("2nd May 2025");
        assert_eq!(ts3.precision, TimePrecision::Day);
        assert!(ts3.human_readable.contains("May 02, 2025") || ts3.human_readable.contains("May 2, 2025"));
    }

    #[test]
    fn test_parse_time_only() {
        let ts_1pm = detect_timestamp("1pm").expect("1pm");
        assert_eq!(ts_1pm.precision, TimePrecision::Hour);

        let ts_1_00_pm = detect_timestamp("1:00 pm").expect("1:00 pm");
        assert_eq!(ts_1_00_pm.precision, TimePrecision::Minute);
    }

    #[test]
    fn test_relative_time_1pm_rounding() {
        // Target: 13:00:00 (1 PM)
        let target = Utc.with_ymd_and_hms(2026, 9, 10, 13, 0, 0).unwrap();

        // At 2:10 PM (14:10:00) -> 70m diff -> rounds to 1H -> "-1H"
        let now_2_10 = Utc.with_ymd_and_hms(2026, 9, 10, 14, 10, 0).unwrap();
        let rel_2_10 = format_relative_time_from(&target, &now_2_10, TimePrecision::Hour);
        assert_eq!(rel_2_10, "-1H");

        // At 2:31 PM (14:31:00) -> 91m diff -> rounds to 2H -> "-2H"
        let now_2_31 = Utc.with_ymd_and_hms(2026, 9, 10, 14, 31, 0).unwrap();
        let rel_2_31 = format_relative_time_from(&target, &now_2_31, TimePrecision::Hour);
        assert_eq!(rel_2_31, "-2H");
    }

    #[test]
    fn test_relative_time_1_00_pm_minutes() {
        // Target: 13:00:00 (1:00 PM) with Minute precision
        let target = Utc.with_ymd_and_hms(2026, 9, 10, 13, 0, 0).unwrap();
        // At 2:10 PM (14:10:00) -> 1h 10m -> "-00:01:10"
        let now_2_10 = Utc.with_ymd_and_hms(2026, 9, 10, 14, 10, 0).unwrap();
        let rel = format_relative_time_from(&target, &now_2_10, TimePrecision::Minute);
        assert_eq!(rel, "-00:01:10");
    }

    #[test]
    fn test_relative_time_over_90_days() {
        let now = Utc.with_ymd_and_hms(2026, 9, 10, 12, 0, 0).unwrap();
        // 100 days ago -> 3 months 10 days
        let target = now - chrono::Duration::days(100);
        let rel = format_relative_time_from(&target, &now, TimePrecision::Day);
        assert_eq!(rel, "-00:03:10");
    }

    #[test]
    fn test_relative_time_under_90_days_day_precision() {
        let now = Utc.with_ymd_and_hms(2026, 9, 10, 12, 0, 0).unwrap();
        let target = now + chrono::Duration::days(45);
        let rel = format_relative_time_from(&target, &now, TimePrecision::Day);
        assert_eq!(rel, "+45D");
    }
}

