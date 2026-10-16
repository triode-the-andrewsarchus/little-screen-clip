//! Read-only telemetry views derived from scoped, typed observations.
mod derive;
mod ui;
use super::{
    Document, Relation,
    attributes::{self, Context, Family},
    collector::Reads,
    model::{Fact, Outcome, Value},
};
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
};
pub(super) use ui::{inspector, paint};
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(in crate::file_manager) enum Kind {
    Hwmon,
    Thermal,
    Power,
    Led,
    Backlight,
    Powercap,
    Regulator,
    Devfreq,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::file_manager) struct Point {
    pub label: String,
    pub display: String,
    pub value: Option<f64>,
    pub unit: String,
    #[serde(with = "super::path_serde")]
    pub source: PathBuf,
    pub observed_ms: u64,
    pub retained: bool,
    pub outcome: Outcome,
}
impl Point {
    fn fact(fact: &Fact, label: impl Into<String>) -> Self {
        let (value, unit) = match &fact.semantic {
            Some(Value::Measurement { value, unit }) => (Some(*value), unit.clone()),
            Some(Value::Integer(value)) => (Some(*value as f64), "raw".into()),
            _ => (None, String::new()),
        };
        Self {
            label: label.into(),
            display: fact
                .value
                .clone()
                .unwrap_or_else(|| fact.outcome.to_string()),
            value,
            unit,
            source: fact.source.clone(),
            observed_ms: fact.observed_ms,
            retained: fact.retained,
            outcome: fact.outcome.clone(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::file_manager) struct Series {
    pub label: String,
    pub current: Point,
    pub markers: Vec<Point>,
    pub range: Option<(f64, f64)>,
    pub notes: Vec<String>,
    pub alarm: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::file_manager) struct Telemetry {
    pub kind: Kind,
    #[serde(with = "super::path_serde")]
    pub root: PathBuf,
    pub role: String,
    pub summary: String,
    pub series: Vec<Series>,
    pub coverage: Outcome,
}
fn named<'a>(document: &'a Document, root: &Path, name: &str) -> Option<&'a Fact> {
    document
        .facts
        .iter()
        .find(|fact| fact.source == root.join(name))
}
fn text<'a>(document: &'a Document, root: &Path, name: &str) -> Option<&'a str> {
    named(document, root, name)
        .filter(|fact| fact.outcome == Outcome::Observed && !fact.retained)?
        .value
        .as_deref()
}
fn present(fact: &Fact) -> bool {
    fact.outcome != Outcome::Missing || fact.value.is_some()
}
pub(super) fn assemble(document: &Document, kind: Kind, root: &Path) -> Option<Telemetry> {
    let (role, mut summary, mut series) = match kind {
        Kind::Hwmon => derive::hwmon(document, root),
        Kind::Thermal => derive::thermal(document, root),
        Kind::Power => derive::power(document, root),
        Kind::Powercap => derive::powercap(document, root),
        Kind::Regulator => derive::regulator(document, root),
        Kind::Devfreq => derive::devfreq(document, root),
        Kind::Led | Kind::Backlight => {
            derive::brightness(document, root, matches!(kind, Kind::Backlight))
        }
    };
    if let Some(fact) = document
        .facts
        .iter()
        .find(|fact| fact.source == document.path && fact.source.parent() == Some(root))
    {
        let current = Point::fact(fact, &fact.label);
        let matched = series.iter().find(|row| {
            std::iter::once(&row.current)
                .chain(&row.markers)
                .any(|point| point.source == current.source && point.unit == current.unit)
        });
        let alarm = matched.is_some_and(|row| row.alarm);
        let markers = matched
            .map(|row| {
                std::iter::once(&row.current)
                    .chain(&row.markers)
                    .filter(|point| point.source != current.source)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        summary = format!("{}: {}", current.label, current.display);
        series = vec![derive::series(
            current.label.clone(),
            current,
            markers,
            vec!["Source attribute; plotted span uses only compatible observed values".into()],
            alarm,
        )];
    }
    if series.is_empty() && summary.is_empty() {
        return None;
    }
    let coverage = document
        .facts
        .iter()
        .filter(|fact| {
            fact.source.parent() == Some(root)
                || fact.source == root
                || fact.label == "Collection completion"
        })
        .find(|fact| !matches!(fact.outcome, Outcome::Observed | Outcome::Missing))
        .map_or(Outcome::Observed, |fact| fact.outcome.clone());
    Some(Telemetry {
        kind,
        root: root.into(),
        role,
        summary,
        series,
        coverage,
    })
}
fn kind(context: &Context) -> Option<Kind> {
    match context.family {
        Family::Hwmon => Some(Kind::Hwmon),
        Family::Thermal => Some(Kind::Thermal),
        Family::PowerSupply => Some(Kind::Power),
        Family::Leds if context.direct => Some(Kind::Led),
        Family::Backlight if context.direct => Some(Kind::Backlight),
        Family::Powercap if context.direct => Some(Kind::Powercap),
        Family::Regulator if context.direct => Some(Kind::Regulator),
        Family::Devfreq if context.direct => Some(Kind::Devfreq),
        _ => None,
    }
}
pub(super) fn seed(document: &mut Document, root: &Path, context: &Context) {
    if let Some(kind) = kind(context) {
        document.telemetry = assemble(document, kind, root);
    }
}
pub(super) fn collect(document: &mut Document, root: &Path, context: &Context, reads: &mut Reads) {
    let Some(kind) = kind(context) else {
        return;
    };
    if !document.interface_directory {
        match kind {
            Kind::Hwmon => sensor_context(document, root, context, reads),
            Kind::Thermal
            | Kind::Power
            | Kind::Led
            | Kind::Backlight
            | Kind::Powercap
            | Kind::Regulator
            | Kind::Devfreq => {
                for rule in attributes::rules(context) {
                    reads.observe(document, root, &rule);
                }
            }
        }
    }
    if matches!(kind, Kind::Thermal) {
        thermal_links(document, root);
    }
    document.telemetry = assemble(document, kind, root);
    if document.interface_directory {
        if let Some(data) = &document.telemetry {
            document.title = data.role.clone();
        }
    }
}
fn sensor_context(document: &mut Document, root: &Path, context: &Context, reads: &mut Reads) {
    if let Some(rule) = attributes::rule(context, "name") {
        reads.observe(document, root, &rule);
    }
    let name = document
        .path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let Some((channel, kind)) = attributes::sensor_channel(&name) else {
        return;
    };
    let channel = channel.to_string();
    let kind = kind.to_string();
    let mut names = vec![
        format!("{channel}_label"),
        format!("{channel}_input"),
        format!("{channel}_min"),
        format!("{channel}_max"),
    ];
    names.extend(
        derive::ALARM_FIELDS
            .iter()
            .map(|field| format!("{channel}_{field}")),
    );
    if kind == "power" {
        names.push(format!("{channel}_average"));
    }
    if kind == "temp" {
        names.extend([
            format!("{channel}_crit"),
            format!("{channel}_lcrit"),
            format!("{channel}_emergency"),
        ]);
    }
    if kind == "pwm" {
        names.extend([channel.clone(), format!("{channel}_enable")]);
    }
    for name in names {
        if let Some(mut rule) = attributes::rule(context, &name) {
            super::collector::sensor_metadata(context, root, &mut rule, reads);
            reads.observe(document, root, &rule);
        }
    }
}
fn thermal_links(document: &mut Document, root: &Path) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            document.facts.push(Fact::failure(
                "Thermal relationship inventory",
                root.into(),
                error,
            ));
            return;
        }
    };
    for (index, entry) in entries.enumerate().take(129) {
        if index == 128 {
            document.facts.push(Fact::failure(
                "Thermal relationship inventory",
                root.into(),
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Thermal relationship inventory capped at 128 entries",
                ),
            ));
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                document.facts.push(Fact::failure(
                    "Thermal relationship inventory",
                    root.into(),
                    error,
                ));
                continue;
            }
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let cooling = name
            .strip_prefix("cdev")
            .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()));
        if !cooling && name != "device" && name != "hwmon" {
            continue;
        }
        match entry.path().canonicalize() {
            Ok(target)
                if !document
                    .relations
                    .iter()
                    .any(|relation| relation.target == target) =>
            {
                document.relations.push(Relation::new(
                    if cooling {
                        "Associated cooling device"
                    } else {
                        "Thermal hardware/sensors"
                    },
                    target,
                ))
            }
            Err(error) => {
                document
                    .facts
                    .push(Fact::failure("Thermal relationship", entry.path(), error))
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focused_temperature_reads_documented_channel_alarms_without_inventing_a_threshold_alert() {
        for flag in ["crit_alarm", "emergency_alarm", "max_alarm", "min_alarm"] {
            let attribute = format!("temp1_{flag}");
            let (_temp, root, mut reads, mut document) =
                super::super::collector::registered_fixture(
                    "hwmon",
                    "hwmon0",
                    &[
                        ("name", "fixture-chip"),
                        ("temp1_label", "Board sensor"),
                        ("temp1_input", "30000"),
                        ("temp1_crit", "90000"),
                        (&attribute, "1"),
                        ("temp1_lcrit_alarm", "1"),
                    ],
                );
            document.path = root.join("temp1_input");
            document.facts.clear();
            collect(&mut document, &root, &Context::discover(&root), &mut reads);
            let data = document.telemetry.as_ref().unwrap();
            assert_eq!(data.series.len(), 1);
            assert_eq!(data.series[0].current.value, Some(30.0));
            assert!(
                data.series[0].alarm,
                "reported {flag} must remain visible for the input"
            );
            assert!(
                document
                    .facts
                    .iter()
                    .any(|fact| fact.source == root.join(&attribute)
                        && fact.outcome == Outcome::Observed)
            );
            assert!(
                !reads
                    .attempts()
                    .any(|path| path == root.join("temp1_lcrit_alarm"))
            );
        }
    }
}
