//! Read-only iCalendar events and wall-clock timelines. Time zones are never inferred.
use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use ical::{parser::ical::component::IcalEvent, property::Property};
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BinaryHeap},
};

/// Maximum encoded calendar size.
pub const MAX_BYTES: usize = 8 * 1024 * 1024;
/// Separate coordinate systems. Named zones retain their local wall times and rules.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Basis {
    /// Date-only, all-day values.
    Date,
    /// Explicit UTC (`Z`) values.
    Utc,
    /// Local values without a time zone.
    Floating,
    /// A declared TZID, without converting or applying embedded transition rules.
    Named(String),
}
impl Basis {
    /// Human-readable coordinate-system label.
    pub fn label(&self) -> String {
        match self {
            Self::Date => "All-day dates".into(),
            Self::Utc => "UTC".into(),
            Self::Floating => "Floating local times".into(),
            Self::Named(name) => format!("Local times · {}", clipped(name, 120)),
        }
    }
}
/// A basic DATE or DATE-TIME value.
#[derive(Clone, Debug)]
pub struct Moment {
    /// Its explicit coordinate system.
    pub basis: Basis,
    /// Ordinal Gregorian days × 86,400 plus time-of-day seconds, not a Unix timestamp.
    pub tick: i64,
    /// Readable source date and time.
    pub label: String,
}
/// One VEVENT, including invalid or unplaced records for inspection.
pub struct Event {
    /// Summary or an untitled placeholder.
    pub title: String,
    /// Calendar name when supplied.
    pub calendar: String,
    /// Start value, when readable.
    pub start: Option<Moment>,
    /// Exclusive end, when readable in the same basis.
    pub end: Option<Moment>,
    /// Whether the end was derived from a duration or the default all-day span.
    pub inferred_end: bool,
    /// Decoded description.
    pub description: String,
    /// Decoded location.
    pub location: String,
    /// Declared cancellation state.
    pub cancelled: bool,
    /// All original properties and alarm declarations, bounded for display.
    pub properties: Vec<(String, String)>,
    /// Actual issues and unexpanded recurrence information.
    pub notes: Vec<String>,
}
/// A positioned event in one compatible time basis.
#[derive(Clone, Debug)]
pub struct Bar {
    /// Original event index, retained through asynchronous loading.
    pub event: usize,
    /// Display title, bounded during parsing.
    pub title: String,
    /// Declared cancellation state.
    pub cancelled: bool,
    /// Overlap lane.
    pub lane: usize,
    /// Start coordinate.
    pub start: i64,
    /// Exclusive end, or the start for an instant.
    pub end: i64,
}
/// A precomputed timeline; overlapping events occupy distinct lanes.
#[derive(Clone)]
pub struct Plan {
    /// Shared time basis.
    pub basis: Basis,
    /// First coordinate.
    pub first: i64,
    /// Last coordinate (with context for a single instant).
    pub last: i64,
    /// Number of overlap lanes.
    pub lanes: usize,
    /// Start-ordered event bars.
    pub bars: Vec<Bar>,
}
/// One or more VCALENDAR objects from a file.
pub struct Document {
    /// Supplied calendar names.
    pub names: Vec<String>,
    /// Original header properties, prefixed by calendar number.
    pub properties: Vec<(String, String)>,
    /// Events in source order.
    pub events: Vec<Event>,
    /// Other component count, available in the source preview.
    pub other_components: usize,
    /// Time-zone definition count, retained as source metadata.
    pub timezones: usize,
    /// Sorted event indices for the agenda.
    pub order: Vec<usize>,
    /// Independent precomputed time-basis timelines.
    pub plans: Vec<Plan>,
    /// UTF-8 source preview (at most 1 MiB).
    pub source: String,
    /// Whether the source preview is incomplete.
    pub source_truncated: bool,
}
impl Document {
    /// Parses events and their declared properties without expanding recurrence or alarms.
    ///
    /// # Errors
    /// Rejects invalid UTF-8, malformed components and exceeded input/record budgets.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BYTES {
            return Err("Calendar exceeds the 8 MiB opening limit".into());
        }
        let text = std::str::from_utf8(bytes).map_err(|e| format!("Calendar is not UTF-8: {e}"))?;
        if text.lines().take(131_073).count() > 131_072 {
            return Err("Calendar has too many source lines".into());
        }
        let input = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut doc = Self {
            names: Vec::new(),
            properties: Vec::new(),
            events: Vec::new(),
            other_components: 0,
            timezones: 0,
            order: Vec::new(),
            plans: Vec::new(),
            source: text[..text.floor_char_boundary(1024 * 1024)].into(),
            source_truncated: text.len() > 1024 * 1024,
        };
        for (index, result) in ical::IcalParser::new(input.as_bytes()).enumerate() {
            if index >= 16 {
                return Err("Calendar has too many VCALENDAR objects".into());
            }
            let calendar = result.map_err(|e| format!("Invalid calendar: {e}"))?;
            let name = value(&calendar.properties, "X-WR-CALNAME")
                .or_else(|| value(&calendar.properties, "NAME"))
                .map(|name| clipped(&unescape(name), 1024))
                .unwrap_or_default();
            if !name.is_empty() {
                doc.names.push(name.clone());
            }
            doc.properties.extend(
                fields(&calendar.properties)
                    .into_iter()
                    .map(|(key, val)| (format!("Calendar {} / {key}", index + 1), val)),
            );
            doc.other_components += calendar.todos.len()
                + calendar.journals.len()
                + calendar.free_busys.len()
                + calendar.alarms.len();
            doc.timezones += calendar.timezones.len();
            for zone in &calendar.timezones {
                doc.properties.extend(
                    fields(&zone.properties)
                        .into_iter()
                        .map(|(key, val)| (format!("Time zone / {key}"), val)),
                );
            }
            for event in calendar.events {
                if doc.events.len() >= 8192 {
                    return Err("Calendar exceeds the 8,192 event limit".into());
                }
                doc.events.push(event_record(event, &name));
            }
        }
        if doc.properties.is_empty() && doc.events.is_empty() {
            return Err("No VCALENDAR data found".into());
        }
        doc.order = (0..doc.events.len()).collect();
        doc.order.sort_by_key(|index| {
            doc.events[*index]
                .start
                .as_ref()
                .map(|time| (&time.basis, time.tick))
        });
        doc.plans = plans(&doc.events);
        Ok(doc)
    }
}
fn value<'a>(properties: &'a [Property], name: &str) -> Option<&'a str> {
    properties
        .iter()
        .find(|p| p.name == name)
        .and_then(|p| p.value.as_deref())
}
fn fields(properties: &[Property]) -> Vec<(String, String)> {
    properties
        .iter()
        .map(|p| {
            let mut name = p.name.clone();
            for (key, values) in p.params.as_deref().unwrap_or_default() {
                name.push_str(&format!(";{key}={}", values.join(",")));
            }
            let raw = p.value.as_deref().unwrap_or("");
            (clipped(&name, 512), clipped(raw, 8192))
        })
        .collect()
}
fn event_record(raw: IcalEvent, calendar: &str) -> Event {
    let properties = &raw.properties;
    let mut notes = Vec::new();
    let start = properties
        .iter()
        .find(|p| p.name == "DTSTART")
        .and_then(|p| match moment(p) {
            Ok(value) => Some(value),
            Err(error) => {
                notes.push(error);
                None
            }
        });
    if start.is_none() && value(properties, "DTSTART").is_none() {
        notes.push("No DTSTART value is supplied.".into());
    }
    for name in ["DTSTART", "DTEND", "DURATION"] {
        if properties.iter().filter(|p| p.name == name).count() > 1 {
            notes.push(format!("Multiple {name} properties; the first is shown."));
        }
    }
    if value(properties, "RRULE").is_some() || value(properties, "RDATE").is_some() {
        notes.push("Recurrence is shown as stored; repeated occurrences are not expanded.".into());
    }
    let (end, inferred_end) = event_end(properties, start.as_ref(), &mut notes);
    let mut all_fields = fields(properties);
    for (index, alarm) in raw.alarms.iter().enumerate() {
        all_fields.extend(
            fields(&alarm.properties)
                .into_iter()
                .map(|(key, val)| (format!("Declared alarm {} / {key}", index + 1), val)),
        );
    }
    Event {
        title: value(properties, "SUMMARY")
            .map(|title| clipped(&unescape(title), 1024))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "(Untitled event)".into()),
        calendar: calendar.into(),
        start,
        end,
        inferred_end,
        description: value(properties, "DESCRIPTION")
            .map(|text| clipped(&unescape(text), 64 * 1024))
            .unwrap_or_default(),
        location: value(properties, "LOCATION")
            .map(|text| clipped(&unescape(text), 4096))
            .unwrap_or_default(),
        cancelled: value(properties, "STATUS").is_some_and(|s| s.eq_ignore_ascii_case("CANCELLED")),
        properties: all_fields,
        notes,
    }
}
fn moment(property: &Property) -> Result<Moment, String> {
    let raw = property.value.as_deref().ok_or("Missing date value")?;
    let parameter = |name: &str| {
        property
            .params
            .as_deref()
            .unwrap_or_default()
            .iter()
            .find(|(key, _)| key == name)
            .and_then(|(_, values)| values.first())
            .map(String::as_str)
    };
    let tz = parameter("TZID");
    if tz.is_some_and(|name| name.len() > 1024) {
        return Err("TZID exceeds the reader limit; its source property is retained.".into());
    }
    let is_date =
        parameter("VALUE") == Some("DATE") || (raw.len() == 8 && parameter("VALUE").is_none());
    if is_date {
        if tz.is_some() {
            return Err("A DATE value cannot carry a TZID.".into());
        }
        let date = NaiveDate::parse_from_str(raw, "%Y%m%d")
            .map_err(|_| format!("Unreadable date: {raw}"))?;
        return Ok(Moment {
            basis: Basis::Date,
            tick: i64::from(date.num_days_from_ce()) * 86400,
            label: date.format("%Y-%m-%d").to_string(),
        });
    }
    let utc = raw.ends_with('Z');
    if utc && tz.is_some() {
        return Err("A UTC date-time cannot carry a TZID.".into());
    }
    let local =
        NaiveDateTime::parse_from_str(raw.strip_suffix('Z').unwrap_or(raw), "%Y%m%dT%H%M%S")
            .map_err(|_| format!("Unreadable date-time: {raw}"))?;
    let basis = if utc {
        Basis::Utc
    } else {
        tz.map(|name| Basis::Named(name.into()))
            .unwrap_or(Basis::Floating)
    };
    Ok(Moment {
        basis,
        tick: i64::from(local.date().num_days_from_ce()) * 86400
            + i64::from(local.time().num_seconds_from_midnight()),
        label: local.format("%Y-%m-%d %H:%M:%S").to_string(),
    })
}
fn event_end(
    properties: &[Property],
    start: Option<&Moment>,
    notes: &mut Vec<String>,
) -> (Option<Moment>, bool) {
    let Some(start) = start else {
        return (None, false);
    };
    if let Some(property) = properties.iter().find(|p| p.name == "DTEND") {
        if value(properties, "DURATION").is_some() {
            notes.push("Both DTEND and DURATION are supplied; DTEND is shown.".into());
        }
        return match moment(property) {
            Ok(end) if end.basis == start.basis && end.tick > start.tick => (Some(end), false),
            Ok(_) => {
                notes.push(
                    "The end is not later in the same time basis; only the start is placed.".into(),
                );
                (None, false)
            }
            Err(error) => {
                notes.push(error);
                (None, false)
            }
        };
    }
    let seconds = if let Some(raw) = value(properties, "DURATION") {
        match duration(raw) {
            Some((seconds, has_time)) if !(start.basis == Basis::Date && has_time) => {
                if matches!(start.basis, Basis::Named(_)) && has_time {
                    notes.push("The duration is retained; an end requires applying the named time zone rules.".into());
                    return (None, false);
                }
                seconds
            }
            _ => {
                notes.push("The supplied event duration cannot be placed.".into());
                return (None, false);
            }
        }
    } else if start.basis == Basis::Date {
        86400
    } else {
        return (None, false);
    };
    let end = start
        .tick
        .checked_add(seconds)
        .and_then(|tick| at_tick(tick, &start.basis));
    if end.is_none() {
        notes.push("The inferred end is outside the supported date range.".into());
    }
    (end, true)
}
fn duration(raw: &str) -> Option<(i64, bool)> {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let pattern = PATTERN.get_or_init(|| {
        regex::Regex::new(r"^\+?P(?:(\d+)W|(?:(\d+)D)?(?:T(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?)?)$")
            .expect("fixed duration pattern")
    });
    let captures = pattern.captures(raw)?;
    let mut seconds = 0i64;
    let mut any = false;
    for (index, scale) in [(1, 604800i64), (2, 86400), (3, 3600), (4, 60), (5, 1)] {
        if let Some(value) = captures.get(index) {
            any = true;
            seconds =
                seconds.checked_add(value.as_str().parse::<i64>().ok()?.checked_mul(scale)?)?;
        }
    }
    (any && seconds > 0).then_some((
        seconds,
        captures.get(3).is_some() || captures.get(4).is_some() || captures.get(5).is_some(),
    ))
}
fn at_tick(tick: i64, basis: &Basis) -> Option<Moment> {
    let days = i32::try_from(tick.div_euclid(86400)).ok()?;
    let date = NaiveDate::from_num_days_from_ce_opt(days)?;
    let time = NaiveTime::from_num_seconds_from_midnight_opt(tick.rem_euclid(86400) as u32, 0)?;
    let label = if *basis == Basis::Date {
        date.format("%Y-%m-%d").to_string()
    } else {
        date.and_time(time).format("%Y-%m-%d %H:%M:%S").to_string()
    };
    Some(Moment {
        basis: basis.clone(),
        tick,
        label,
    })
}
/// Formats an ordinal timeline coordinate in its own wall-clock basis.
pub fn tick_label(tick: i64, dates_only: bool) -> String {
    at_tick(
        tick,
        &if dates_only {
            Basis::Date
        } else {
            Basis::Floating
        },
    )
    .map(|moment| moment.label)
    .unwrap_or_else(|| "Outside date range".into())
}
fn plans(events: &[Event]) -> Vec<Plan> {
    let mut groups: BTreeMap<Basis, Vec<(usize, i64, i64)>> = BTreeMap::new();
    for (index, event) in events.iter().enumerate() {
        if let Some(start) = &event.start {
            groups.entry(start.basis.clone()).or_default().push((
                index,
                start.tick,
                event.end.as_ref().map_or(start.tick, |end| end.tick),
            ));
        }
    }
    groups
        .into_iter()
        .map(|(basis, mut records)| {
            records.sort_by_key(|(index, start, end)| (*start, *end, *index));
            let first = records[0].1;
            let last = records
                .iter()
                .map(|(_, _, end)| *end)
                .max()
                .unwrap_or(first)
                .max(first + 3600);
            let mut heap: BinaryHeap<Reverse<(i64, usize)>> = BinaryHeap::new();
            let mut lanes = 0;
            let mut bars = Vec::new();
            for (event, start, end) in records {
                let lane = if heap
                    .peek()
                    .is_some_and(|Reverse((previous, _))| *previous <= start)
                {
                    heap.pop().unwrap().0.1
                } else {
                    let lane = lanes;
                    lanes += 1;
                    lane
                };
                heap.push(Reverse((end.max(start + 1), lane)));
                bars.push(Bar {
                    title: events[event].title.clone(),
                    cancelled: events[event].cancelled,
                    event,
                    lane,
                    start,
                    end,
                });
            }
            Plan {
                basis,
                first,
                last,
                lanes,
                bars,
            }
        })
        .collect()
}
fn unescape(value: &str) -> String {
    let mut output = String::new();
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            output.push(character);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => output.push('\n'),
            Some(c @ ('\\' | ',' | ';')) => output.push(c),
            Some(c) => {
                output.push('\\');
                output.push(c)
            }
            None => output.push('\\'),
        }
    }
    output
}

fn clipped(text: &str, max: usize) -> String {
    let end = text.floor_char_boundary(max);
    format!(
        "{}{}",
        &text[..end],
        if end < text.len() { "…" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    const CALENDAR: &[u8] = include_bytes!("../tests/fixtures/calendar/harbor.ics");
    #[test]
    fn folded_text_and_calendar_metadata_are_preserved_without_mixing_time_bases() {
        let doc = Document::parse(CALENDAR).unwrap();
        assert_eq!(doc.names, ["Harbor calendar"]);
        assert_eq!(doc.events.len(), 5);
        assert_eq!(doc.other_components, 1);
        assert_eq!(doc.timezones, 1);
        assert_eq!(doc.events[0].title, "Harbor crossing");
        assert_eq!(doc.events[0].location, "North pier, gate 2");
        assert!(doc.events[0].description.contains('\n'));
        assert_eq!(doc.plans.len(), 4);
        assert!(doc.events[1].cancelled);
        assert!(
            doc.events[4]
                .notes
                .iter()
                .any(|note| note.contains("not expanded"))
        );
        assert!(
            doc.events[4]
                .properties
                .iter()
                .any(|(name, _)| name.contains("Declared alarm"))
        );
    }
    #[test]
    fn overlap_lanes_duration_and_default_all_day_end_are_explicit() {
        let doc = Document::parse(CALENDAR).unwrap();
        let utc = doc
            .plans
            .iter()
            .find(|plan| plan.basis == Basis::Utc)
            .unwrap();
        assert_eq!(utc.lanes, 2);
        assert_ne!(utc.bars[0].lane, utc.bars[1].lane);
        assert_eq!(
            doc.events[1].end.as_ref().unwrap().tick - doc.events[1].start.as_ref().unwrap().tick,
            7200
        );
        assert_eq!(doc.events[2].end.as_ref().unwrap().label, "2026-11-12");
        assert!(doc.events[2].inferred_end);
        assert_eq!(doc.events[3].start.as_ref().unwrap().basis, Basis::Floating);
        assert_eq!(
            doc.events[4].start.as_ref().unwrap().basis,
            Basis::Named("Harbor/Local".into())
        );
    }
    #[test]
    fn invalid_dates_stay_inspectable_and_duration_arithmetic_cannot_overflow() {
        let raw=b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nDTSTART:20260230T090000Z\r\nSUMMARY:Invalid day\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let doc = Document::parse(raw).unwrap();
        assert_eq!(doc.events.len(), 1);
        assert!(doc.events[0].start.is_none());
        assert!(!doc.events[0].notes.is_empty());
        assert!(doc.plans.is_empty());
        for input in ["P", "PT", "-PT1H", "PT0S", "P1W2D", "P999999999999999999W"] {
            assert!(duration(input).is_none(), "{input}");
        }
        assert_eq!(duration("P1DT2H30M"), Some((95400, true)));
        assert!(Document::parse(&vec![b'x'; MAX_BYTES + 1]).is_err());
    }
    #[test]
    fn duration_in_a_named_zone_does_not_invent_an_absolute_end() {
        let raw=b"BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nDTSTART;TZID=Harbor/Local:20261113T090000\r\nDURATION:PT1H\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let doc = Document::parse(raw).unwrap();
        assert!(doc.events[0].end.is_none());
        assert!(
            doc.events[0]
                .notes
                .iter()
                .any(|note| note.contains("time zone rules"))
        );
    }
}
