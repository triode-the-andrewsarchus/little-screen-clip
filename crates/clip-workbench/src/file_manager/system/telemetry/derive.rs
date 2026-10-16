use super::{Document, Outcome, Point, Series, Value, named, present, text};
use std::{collections::BTreeSet, path::Path};
pub(super) const ALARM_FIELDS: &[&str] = &[
    "alarm",
    "fault",
    "crit_alarm",
    "emergency_alarm",
    "max_alarm",
    "min_alarm",
];
fn alarm_point(document: &Document, root: &Path, channel: &str) -> Option<Point> {
    ALARM_FIELDS.iter().find_map(|field| {
        point(
            document,
            root,
            &format!("{channel}_{field}"),
            &format!("Reported {field}"),
        )
    })
}
fn pwm_point(document: &Document, root: &Path, channel: &str) -> Option<Point> {
    point(document, root, channel, "PWM duty").or_else(|| {
        [
            ("mode", "Signal mode"),
            ("enable", "Control method"),
            ("freq", "Base frequency"),
        ]
        .iter()
        .find_map(|(field, label)| point(document, root, &format!("{channel}_{field}"), label))
    })
}
fn point(document: &Document, root: &Path, name: &str, label: &str) -> Option<Point> {
    named(document, root, name)
        .filter(|fact| present(fact))
        .map(|fact| {
            let mut point = Point::fact(fact, label);
            if matches!(name, "cur_state" | "max_state") {
                point.unit = "state".into();
            }
            point
        })
}
fn observed_span(current: &Point, markers: &[Point]) -> Option<(f64, f64)> {
    let values = current
        .value
        .into_iter()
        .chain(
            markers
                .iter()
                .filter(|marker| marker.unit == current.unit)
                .filter_map(|marker| marker.value),
        )
        .collect::<Vec<_>>();
    if markers
        .iter()
        .all(|marker| marker.value.is_none() || marker.unit != current.unit)
    {
        return None;
    }
    let min = values.iter().copied().reduce(f64::min)?;
    let max = values.iter().copied().reduce(f64::max)?;
    (min.is_finite() && max.is_finite() && min < max).then_some((min, max))
}
pub(super) fn series(
    label: String,
    current: Point,
    markers: Vec<Point>,
    notes: Vec<String>,
    alarm: bool,
) -> Series {
    let range = observed_span(&current, &markers);
    Series {
        label,
        current,
        markers,
        range,
        notes,
        alarm,
    }
}
pub(super) fn hwmon(document: &Document, root: &Path) -> (String, String, Vec<Series>) {
    let driver = named(document, root, "name")
        .and_then(|fact| fact.value.as_deref())
        .unwrap_or("driver identity unavailable");
    let channels = document
        .facts
        .iter()
        .filter(|fact| fact.source.parent() == Some(root) && present(fact))
        .filter_map(|fact| fact.source.file_name()?.to_str())
        .filter_map(|name| {
            super::super::attributes::sensor_channel(name).map(|(channel, _)| channel.to_string())
        })
        .collect::<BTreeSet<_>>();
    let mut rows = Vec::new();
    for channel in channels {
        let mut current = point(document, root, &format!("{channel}_input"), "Input")
            .or_else(|| point(document, root, &format!("{channel}_average"), "Average"));
        if channel.starts_with("pwm") {
            current = pwm_point(document, root, &channel);
        }
        let Some(mut current) = current.or_else(|| alarm_point(document, root, &channel)) else {
            continue;
        };
        let label = text(document, root, &format!("{channel}_label"))
            .map(str::to_string)
            .unwrap_or_else(|| {
                named(document, root, &format!("{channel}_input"))
                    .map(|fact| {
                        fact.label
                            .split(" · ")
                            .next()
                            .unwrap_or(&channel)
                            .trim_end_matches("_input")
                            .to_string()
                    })
                    .unwrap_or(channel.clone())
            });
        let mut notes = Vec::new();
        let mut alarm = false;
        if current.value.is_none() {
            notes.push(
                "Unit/measurement unavailable or unverified; no normalized value is inferred"
                    .into(),
            );
        }
        let (flags, reported_alarm) = channel_flags(document, root, &channel);
        notes.extend(flags);
        alarm |= reported_alarm;
        let mut markers = ["min", "max", "lcrit", "crit", "emergency"]
            .iter()
            .filter_map(|field| point(document, root, &format!("{channel}_{field}"), field))
            .collect::<Vec<_>>();
        if driver == "nvme" {
            nvme_limits(&mut markers, &mut notes);
        }
        if named(document, root, "name").is_some_and(|fact| fact.retained) {
            notes.push("Driver identity is from the last observation".into());
        }
        if channel.starts_with("pwm")
            && current
                .source
                .file_name()
                .is_some_and(|name| name == channel.as_str())
        {
            normalize_pwm(&mut current, &mut notes);
        }
        let mut row = series(
            format!("{driver} / {label}"),
            current,
            markers,
            notes,
            alarm,
        );
        if channel.starts_with("pwm") && row.current.unit == "%" {
            row.range = Some((0.0, 100.0));
        }
        rows.push(row);
    }
    let summary = format!("{driver} · {} observed channels", rows.len());
    ("Hardware sensors".into(), summary, rows)
}
fn nvme_limits(markers: &mut [Point], notes: &mut Vec<String>) {
    // NVMe default per-sensor thresholds are 0 K / 65535 K. Linux exports
    // their Celsius conversion; these endpoints are not useful plot bounds.
    for marker in markers
        .iter_mut()
        .filter(|marker| matches!(marker.label.as_str(), "min" | "max"))
    {
        let endpoint = marker
            .value
            .is_some_and(|value| (value + 273.15).abs() < 1e-6 || (value - 65261.85).abs() < 1e-6);
        if endpoint {
            notes.push(format!("NVMe {}: {} is a default endpoint encoding, excluded from the operating-limit plot",marker.label,marker.display));
            marker.display.push_str(" (default NVMe endpoint)");
            marker.value = None;
        }
    }
}
fn channel_flags(document: &Document, root: &Path, channel: &str) -> (Vec<String>, bool) {
    let mut notes = Vec::new();
    let mut alarm = false;
    for field in ALARM_FIELDS
        .iter()
        .copied()
        .chain(["enable", "mode", "freq"])
    {
        if let Some(fact) =
            named(document, root, &format!("{channel}_{field}")).filter(|fact| present(fact))
        {
            notes.push(format!(
                "{field}: {}{}",
                fact.value.as_deref().unwrap_or("unavailable"),
                if fact.retained {
                    " (last observation)"
                } else {
                    ""
                }
            ));
            alarm |= ALARM_FIELDS.contains(&field)
                && matches!(fact.semantic, Some(Value::Boolean(true)))
                && !fact.retained
                && fact.outcome == Outcome::Observed;
        }
    }
    (notes, alarm)
}
fn normalize_pwm(point: &mut Point, notes: &mut Vec<String>) {
    if let Some(raw) = point.value.filter(|raw| (0.0..=255.0).contains(raw)) {
        let value = raw * 100.0 / 255.0;
        point.value = Some(value);
        point.unit = "%".into();
        point.display = format!("{value:.1}% ({raw:.0}/255)");
    } else {
        notes.push("Raw PWM is unavailable or outside the documented 0–255 range".into());
    }
}
pub(super) fn thermal(document: &Document, root: &Path) -> (String, String, Vec<Series>) {
    let kind = text(document, root, "type").unwrap_or("type unavailable");
    let mut rows = Vec::new();
    let cooling = root
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with("cooling_device"));
    if cooling {
        if let Some(current) = point(document, root, "cur_state", "Current state") {
            let markers = point(document, root, "max_state", "Maximum state")
                .into_iter()
                .collect();
            let mut row = series(
                kind.into(),
                current,
                markers,
                vec!["Ordinal cooling state; not a percentage or a fan-speed measurement".into()],
                false,
            );
            row.range = row
                .markers
                .first()
                .and_then(|marker| marker.value)
                .filter(|max| *max > 0.0)
                .map(|max| (0.0, max));
            rows.push(row);
        }
    } else if let Some(current) = point(document, root, "temp", "Temperature") {
        let mut markers = Vec::new();
        for fact in document
            .facts
            .iter()
            .filter(|fact| fact.source.parent() == Some(root))
        {
            let Some(name) = fact.source.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(id) = name
                .strip_prefix("trip_point_")
                .and_then(|name| name.strip_suffix("_temp"))
            else {
                continue;
            };
            let label = format!(
                "Trip {id}: {}",
                text(document, root, &format!("trip_point_{id}_type"))
                    .unwrap_or("type unavailable")
            );
            markers.push(Point::fact(fact, label));
        }
        rows.push(series(
            kind.into(),
            current,
            markers,
            vec![
                "Span is based on observed temperature/trip values; no safe range is inferred"
                    .into(),
            ],
            false,
        ));
    }
    let value = rows
        .first()
        .map(|row| row.current.display.as_str())
        .unwrap_or("measurement unavailable");
    let summary = if cooling {
        format!(
            "{kind} · State {value} / {}",
            rows.first()
                .and_then(|row| row.markers.first())
                .map(|point| point.display.as_str())
                .unwrap_or("maximum unavailable")
        )
    } else {
        format!("{kind} · {value}")
    };

    (
        if cooling {
            "Thermal cooling device"
        } else {
            "Thermal zone"
        }
        .into(),
        summary,
        rows,
    )
}
fn capacity_ratio(full: &Point, design: &Point, unit: &str) -> Option<Point> {
    let value = full.value?;
    let denominator = design.value?;
    if full.unit != unit || design.unit != unit || value < 0.0 || denominator <= 0.0 {
        return None;
    }
    let ratio = value / denominator * 100.0;
    if !ratio.is_finite() {
        return None;
    }
    Some(Point {
        label: "Full/design estimate".into(),
        display: format!("{ratio:.1}%"),
        value: Some(ratio),
        unit: "%".into(),
        source: full.source.clone(),
        observed_ms: full.observed_ms.min(design.observed_ms),
        retained: full.retained || design.retained,
        outcome: if full.outcome != Outcome::Observed {
            full.outcome.clone()
        } else {
            design.outcome.clone()
        },
    })
}
fn power_state(document: &Document, root: &Path) -> Option<Series> {
    let mut points = [
        ("status", "Status"),
        ("present", "Present"),
        ("online", "Online"),
        ("health", "Kernel health"),
    ]
    .into_iter()
    .filter_map(|(name, label)| point(document, root, name, label));
    Some(series(
        "Supply state".into(),
        points.next()?,
        points.collect(),
        vec![
            "Reported supply state; kernel health is separate from full/design capacity estimates"
                .into(),
        ],
        false,
    ))
}
pub(super) fn power(document: &Document, root: &Path) -> (String, String, Vec<Series>) {
    let kind = text(document, root, "type").unwrap_or("supply type unavailable");
    let status = text(document, root, "status")
        .or_else(|| text(document, root, "online"))
        .unwrap_or("state unavailable");
    let mut rows = Vec::new();
    if let Some(current) = point(document, root, "capacity", "Reported capacity") {
        let mut row = series("Capacity".into(), current, Vec::new(), Vec::new(), false);
        row.range = Some((0.0, 100.0));
        rows.push(row);
    }
    for (prefix, unit, label) in [("energy", "Wh", "Energy"), ("charge", "Ah", "Charge")] {
        let now = point(document, root, &format!("{prefix}_now"), "Remaining");
        let full = point(document, root, &format!("{prefix}_full"), "Full");
        let design = point(document, root, &format!("{prefix}_full_design"), "Design");
        if let Some(current) = now {
            rows.push(series(
                label.into(),
                current,
                full.clone().into_iter().chain(design.clone()).collect(),
                vec![
                    "Energy and charge are shown separately without inferred voltage conversion"
                        .into(),
                ],
                false,
            ));
        }
        if kind == "Battery" && text(document, root, "present") != Some("No") {
            if let Some(estimate) = full
                .as_ref()
                .zip(design.as_ref())
                .and_then(|(full, design)| capacity_ratio(full, design, unit))
            {
                let mut row=series(format!("{label} full/design ratio"),estimate,full.clone().into_iter().chain(design.clone()).collect(),vec!["Capacity estimate, separate from the kernel health state; values above 100% are preserved".into()],false);
                row.range = Some((0.0, 100.0));
                rows.push(row);
            }
        }
    }
    for (name, label) in [
        ("voltage_now", "Voltage"),
        ("current_now", "Current"),
        ("power_now", "Power"),
        ("temp", "Temperature"),
    ] {
        if let Some(current) = point(document, root, name, label) {
            rows.push(series(label.into(), current, Vec::new(), Vec::new(), false));
        }
    }
    let summary = format!(
        "{kind} · {status}{}",
        rows.first()
            .map(|row| format!(" · {}", row.current.display))
            .unwrap_or_default()
    );
    if let Some(state) = power_state(document, root) {
        rows.insert(0, state);
    }
    ("Power supply".into(), summary, rows)
}

#[cfg(test)]
mod tests {
    use super::super::super::{
        attributes::{Context, Family},
        model::Fact,
    };
    use super::*;
    fn fixture(family: Family, values: &[(&str, &str)]) -> Document {
        let root = match family {
            Family::Hwmon => "/sys/class/hwmon/hwmon_fixture",
            Family::Thermal => "/sys/class/thermal/thermal_zone42",
            _ => "/sys/class/power_supply/BAT0",
        };
        let mut document =
            super::super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        document.path = root.into();
        document.facts.clear();
        let subsystem = match family {
            Family::Hwmon => "hwmon",
            Family::Thermal => "thermal",
            _ => "power_supply",
        };
        let context = Context::from_parts(Path::new(root), Some(subsystem));
        for (name, raw) in values {
            let rule = super::super::super::attributes::rule(&context, name).unwrap();
            let (semantic, display) = super::super::super::attributes::parse(&rule, raw).unwrap();
            let mut fact = Fact::text(&rule.label, display, Path::new(root).join(name));
            fact.raw = Some((*raw).into());
            fact.semantic = Some(semantic);
            document.facts.push(fact);
        }
        document
    }
    #[test]
    fn sensor_labels_and_units_do_not_invent_a_cpu_or_a_safe_temperature() {
        let document = fixture(
            Family::Hwmon,
            &[
                ("name", "fixture-driver"),
                ("temp1_label", "Board"),
                ("temp1_input", "-5000"),
                ("temp1_crit", "90000"),
                ("power1_input", "1500000"),
                ("in0_input", "3300"),
                ("curr1_input", "250"),
                ("fan1_input", "1240"),
                ("energy1_input", "2500000"),
            ],
        );
        let (_, _, rows) = hwmon(&document, &document.path);
        let board = rows
            .iter()
            .find(|row| row.label.ends_with("Board"))
            .unwrap();
        assert_eq!(board.current.value, Some(-5.0));
        assert_eq!(board.current.unit, "°C");
        assert_eq!(board.range, Some((-5.0, 90.0)));
        assert!(!board.alarm);
        assert!(!board.label.contains("CPU"));
        assert!(
            rows.iter()
                .any(|row| row.current.value == Some(1.5) && row.current.unit == "W")
        );
        assert!(rows.iter().any(|row| {
            row.current
                .value
                .is_some_and(|value| (value - 3.3).abs() < 1e-9)
                && row.current.unit == "V"
        }));
        assert!(
            rows.iter()
                .any(|row| row.current.value == Some(0.25) && row.current.unit == "A")
        );
        assert!(
            rows.iter()
                .any(|row| row.current.value == Some(1240.0) && row.current.unit == "RPM")
        );
        assert!(
            rows.iter()
                .any(|row| row.current.value == Some(2.5) && row.current.unit == "J")
        );
    }
    #[test]
    fn reported_emergency_alarm_is_distinct_from_temperature_comparisons_and_retained_flags() {
        let mut document = fixture(
            Family::Hwmon,
            &[
                ("name", "fixture-driver"),
                ("temp1_input", "25000"),
                ("temp1_emergency", "100000"),
                ("temp1_emergency_alarm", "1"),
                ("temp1_emergency_hyst", "95000"),
                ("temp1_lcrit_hyst", "-5000"),
            ],
        );
        let (_, _, rows) = hwmon(&document, &document.path);
        assert!(rows[0].alarm);
        assert!(
            rows[0]
                .notes
                .iter()
                .any(|note| note.contains("emergency_alarm: Yes"))
        );
        let flag = document
            .facts
            .iter_mut()
            .find(|fact| fact.source.ends_with("temp1_emergency_alarm"))
            .unwrap();
        flag.retained = true;
        flag.outcome = Outcome::PermissionDenied;
        let (_, _, rows) = hwmon(&document, &document.path);
        assert!(!rows[0].alarm);
        assert!(
            rows[0]
                .notes
                .iter()
                .any(|note| note.contains("last observation"))
        );
        assert!(
            super::super::super::attributes::rule(
                &Context::from_parts(&document.path, Some("hwmon")),
                "power1_emergency_alarm"
            )
            .is_none()
        );
    }

    #[test]
    fn alarm_only_channel_has_a_status_row_without_a_fabricated_measurement_or_range() {
        let document = fixture(
            Family::Hwmon,
            &[("name", "fixture-driver"), ("temp1_emergency_alarm", "1")],
        );
        let (_, _, rows) = hwmon(&document, &document.path);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].alarm);
        assert_eq!(rows[0].current.label, "Reported emergency_alarm");
        assert_eq!(rows[0].current.display, "Yes");
        assert!(rows[0].current.value.is_none());
        assert!(rows[0].range.is_none());
        assert_eq!(
            rows[0].current.source,
            document.path.join("temp1_emergency_alarm")
        );
    }

    #[test]
    fn focused_alarm_attribute_preserves_the_reported_alert() {
        let mut document = fixture(
            Family::Hwmon,
            &[("name", "fixture-driver"), ("temp1_emergency_alarm", "1")],
        );
        let root = document.path.clone();
        document.path = root.join("temp1_emergency_alarm");
        let data = super::super::assemble(&document, super::super::Kind::Hwmon, &root).unwrap();
        assert_eq!(data.series.len(), 1);
        assert!(data.series[0].alarm);
        assert!(data.series[0].current.value.is_none());
        assert!(data.series[0].range.is_none());
    }

    #[test]
    fn pwm_configuration_has_units_and_is_not_an_alarm() {
        let document = fixture(
            Family::Hwmon,
            &[
                ("name", "fixture-driver"),
                ("pwm1", "128"),
                ("pwm1_mode", "1"),
                ("pwm1_enable", "2"),
                ("pwm1_freq", "25000"),
            ],
        );
        let (_, _, rows) = hwmon(&document, &document.path);
        assert_eq!(rows[0].current.unit, "%");
        assert!(!rows[0].alarm);
        assert!(rows[0].notes.iter().any(|note| note.contains("mode: PWM")));
        assert!(rows[0].notes.iter().any(|note| note.starts_with("freq:")));
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.source.ends_with("pwm1_freq")
                    && fact.semantic
                        == Some(Value::Measurement {
                            value: 25000.0,
                            unit: "Hz".into()
                        }))
        );
    }

    #[test]
    fn pwm_frequency_only_is_never_normalized_as_a_duty_percentage() {
        let document = fixture(
            Family::Hwmon,
            &[("name", "fixture-driver"), ("pwm1_freq", "100")],
        );
        let (_, _, rows) = hwmon(&document, &document.path);
        assert_eq!(rows[0].current.label, "Base frequency");
        assert_eq!(rows[0].current.value, Some(100.0));
        assert_eq!(rows[0].current.unit, "Hz");
        assert!(rows[0].range.is_none());
        assert!(!rows[0].alarm);
        let context = Context::from_parts(&document.path, Some("hwmon"));
        for (name, raw) in [
            ("pwm1_freq", "-1"),
            ("pwm1_enable", "-1"),
            ("pwm1_mode", "2"),
        ] {
            let rule = super::super::super::attributes::rule(&context, name).unwrap();
            assert!(super::super::super::attributes::parse(&rule, raw).is_err());
        }
        assert!(super::super::super::attributes::rule(&context, "temp1_freq").is_none());
    }

    #[test]
    fn missing_limits_and_opaque_temperatures_do_not_get_normalized_ranges() {
        let mut document = fixture(
            Family::Hwmon,
            &[("name", "fixture"), ("temp1_input", "40000")],
        );
        assert!(hwmon(&document, &document.path).2[0].range.is_none());
        let fact = document
            .facts
            .iter_mut()
            .find(|fact| fact.source.ends_with("temp1_input"))
            .unwrap();
        fact.semantic = Some(Value::Text("40000".into()));
        let row = hwmon(&document, &document.path).2.remove(0);
        assert!(row.current.value.is_none());
        assert!(row.range.is_none());
    }
    #[test]
    fn only_fresh_alarm_observations_mark_the_current_reading() {
        let mut document = fixture(
            Family::Hwmon,
            &[
                ("name", "fixture"),
                ("temp1_input", "90000"),
                ("temp1_alarm", "1"),
            ],
        );
        assert!(hwmon(&document, &document.path).2[0].alarm);
        document
            .facts
            .iter_mut()
            .find(|fact| fact.source.ends_with("temp1_alarm"))
            .unwrap()
            .retained = true;
        assert!(!hwmon(&document, &document.path).2[0].alarm);
    }
    #[test]
    fn cooling_state_schema_rejects_negative_fractional_and_overflow_values() {
        let context = Context::from_parts(
            Path::new("/sys/class/thermal/cooling_device0"),
            Some("thermal"),
        );
        for name in ["cur_state", "max_state"] {
            let rule = super::super::super::attributes::rule(&context, name).unwrap();
            for invalid in ["-1", "1.5", "18446744073709551616", "unknown"] {
                assert!(super::super::super::attributes::parse(&rule, invalid).is_err());
            }
            for raw in ["0", "5", "18446744073709551615"] {
                let (value, display) = super::super::super::attributes::parse(&rule, raw).unwrap();
                assert_eq!(value, Value::Integer(raw.parse().unwrap()));
                assert_eq!(display, raw);
            }
        }
    }
    #[test]
    fn cooling_states_are_ordinal_and_trip_types_are_retained() {
        let mut document = fixture(
            Family::Thermal,
            &[
                ("type", "Processor"),
                ("cur_state", "2"),
                ("max_state", "5"),
            ],
        );
        document.path = "/sys/class/thermal/cooling_device0".into();
        for fact in &mut document.facts {
            fact.source = document.path.join(fact.source.file_name().unwrap());
        }
        let row = thermal(&document, &document.path).2.remove(0);
        assert_eq!(row.current.value, Some(2.0));
        assert_eq!(row.current.unit, "state");
        assert_eq!(row.range, Some((0.0, 5.0)));
        assert!(!row.current.display.contains('%'));
        let zone = fixture(
            Family::Thermal,
            &[
                ("type", "fixture"),
                ("temp", "61000"),
                ("trip_point_0_temp", "80000"),
                ("trip_point_0_type", "passive"),
            ],
        );
        let row = thermal(&zone, &zone.path).2.remove(0);
        assert_eq!(row.markers[0].label, "Trip 0: passive");
        assert_eq!(row.range, Some((61.0, 80.0)));
    }
    #[test]
    fn power_status_strip_paints_reported_fields_in_both_appearances() {
        use little_clip_core::{config::WorkbenchAppearance, palette::AppPalette};
        let document = fixture(
            Family::PowerSupply,
            &[
                ("type", "Battery"),
                ("status", "Discharging"),
                ("present", "1"),
                ("online", "0"),
                ("health", "Good"),
                ("capacity", "82"),
                ("energy_now", "42000000"),
            ],
        );
        let data =
            super::super::assemble(&document, super::super::Kind::Power, &document.path).unwrap();
        for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
            let palette = AppPalette::for_appearance(appearance);
            let context = egui::Context::default();
            let output = context.run(egui::RawInput::default(), |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    super::super::ui::paint(
                        ui,
                        egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(1500.0, 52.0)),
                        &data,
                        crate::ui_appearance::WorkbenchColors::from_palette(&palette),
                        &palette,
                    );
                });
            });
            let text = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            for expected in [
                "Status: Discharging",
                "Present: Yes",
                "Online: No",
                "Kernel health: Good",
                "Reported capacity: 82 %",
                "Remaining: 42 Wh",
            ] {
                assert!(text.contains(expected), "missing {expected}: {text}");
            }
        }
    }
    #[test]
    fn power_status_is_visible_without_numeric_properties_and_keeps_source_provenance() {
        let mut document = fixture(
            Family::PowerSupply,
            &[
                ("type", "USB"),
                ("status", "Not charging"),
                ("present", "1"),
                ("online", "0"),
                ("health", "Good"),
            ],
        );
        let health = document
            .facts
            .iter_mut()
            .find(|fact| fact.source.ends_with("health"))
            .unwrap();
        health.retained = true;
        health.observed_ms = 123;
        health.outcome = Outcome::PermissionDenied;
        let (_, summary, rows) = power(&document, &document.path);
        assert_eq!(summary, "USB · Not charging");
        assert_eq!(rows.len(), 1);
        let state = &rows[0];
        assert_eq!(state.current.display, "Not charging");
        assert!(state.range.is_none());
        assert!(!state.alarm);
        assert_eq!(state.markers.len(), 3);
        assert_eq!(state.markers[0].display, "Yes");
        assert_eq!(state.markers[1].display, "No");
        let health = &state.markers[2];
        assert!(health.retained);
        assert_eq!(health.observed_ms, 123);
        assert_eq!(health.outcome, Outcome::PermissionDenied);
        assert_eq!(health.source, document.path.join("health"));
        assert!(state.markers.iter().all(|point| point.value.is_none()));
    }
    #[test]
    fn power_state_does_not_invent_missing_fields_or_replace_capacity_series() {
        let document = fixture(
            Family::PowerSupply,
            &[("type", "Battery"), ("capacity", "82"), ("online", "1")],
        );
        let (_, summary, rows) = power(&document, &document.path);
        assert_eq!(summary, "Battery · Yes · 82 %");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "Supply state");
        assert_eq!(rows[0].current.label, "Online");
        assert!(rows[0].markers.is_empty());
        assert_eq!(rows[1].current.value, Some(82.0));
        let empty = fixture(Family::PowerSupply, &[("type", "Mains")]);
        assert!(power(&empty, &empty.path).2.is_empty());
    }
    #[test]
    fn battery_energy_and_charge_ratios_are_independent_and_not_clamped() {
        let document = fixture(
            Family::PowerSupply,
            &[
                ("type", "Battery"),
                ("status", "Discharging"),
                ("capacity", "82"),
                ("energy_now", "42000000"),
                ("energy_full", "51000000"),
                ("energy_full_design", "50000000"),
                ("charge_now", "2000000"),
                ("charge_full", "2500000"),
                ("charge_full_design", "3000000"),
            ],
        );
        let (_, _, rows) = power(&document, &document.path);
        assert_eq!(
            rows.iter()
                .find(|row| row.label == "Energy")
                .unwrap()
                .current
                .unit,
            "Wh"
        );
        assert_eq!(
            rows.iter()
                .find(|row| row.label == "Charge")
                .unwrap()
                .current
                .unit,
            "Ah"
        );
        assert_eq!(
            rows.iter()
                .find(|row| row.label == "Energy full/design ratio")
                .unwrap()
                .current
                .value,
            Some(102.0)
        );
        assert!(
            (rows
                .iter()
                .find(|row| row.label == "Charge full/design ratio")
                .unwrap()
                .current
                .value
                .unwrap()
                - 83.3333333333)
                .abs()
                < 1e-6
        );
    }
    #[test]
    fn capacity_estimates_reject_zero_design_mismatched_units_and_absent_batteries() {
        let mut document = fixture(
            Family::PowerSupply,
            &[
                ("type", "Battery"),
                ("present", "0"),
                ("energy_full", "51000000"),
                ("energy_full_design", "55000000"),
            ],
        );
        let rows = power(&document, &document.path).2;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "Supply state");
        assert_eq!(rows[0].current.display, "No");
        assert!(rows[0].range.is_none());
        let full = Point::fact(
            named(&document, &document.path, "energy_full").unwrap(),
            "Full",
        );
        let mut design = Point::fact(
            named(&document, &document.path, "energy_full_design").unwrap(),
            "Design",
        );
        design.unit = "Ah".into();
        assert!(capacity_ratio(&full, &design, "Wh").is_none());
        design.unit = "Wh".into();
        design.value = Some(0.0);
        assert!(capacity_ratio(&full, &design, "Wh").is_none());
        document
            .facts
            .iter_mut()
            .find(|fact| fact.source.ends_with("energy_full"))
            .unwrap()
            .observed_ms = 100;
        document
            .facts
            .iter_mut()
            .find(|fact| fact.source.ends_with("energy_full_design"))
            .unwrap()
            .observed_ms = 200;
        let full = Point::fact(
            named(&document, &document.path, "energy_full").unwrap(),
            "Full",
        );
        let design = Point::fact(
            named(&document, &document.path, "energy_full_design").unwrap(),
            "Design",
        );
        assert_eq!(
            capacity_ratio(&full, &design, "Wh").unwrap().observed_ms,
            100
        );
    }
    #[test]
    fn retained_sensor_values_rebuild_with_their_original_age_and_failure() {
        let mut previous = fixture(
            Family::Hwmon,
            &[
                ("name", "fixture"),
                ("temp1_input", "44000"),
                ("temp1_max", "90000"),
            ],
        );
        let original = previous
            .facts
            .iter_mut()
            .find(|fact| fact.source.ends_with("temp1_input"))
            .unwrap();
        original.observed_ms = 1000;
        original.origin = "documented sysfs attribute".into();
        original.label = "Renamed sensor · input".into();
        previous.telemetry =
            super::super::assemble(&previous, super::super::Kind::Hwmon, &previous.path);
        let mut current = previous.clone();
        current.telemetry = None;
        current.facts.clear();
        current.last_error = Some("timed out".into());
        current.facts.push(Fact::failure(
            "temp1_input",
            previous.path.join("temp1_input"),
            std::io::Error::from(std::io::ErrorKind::TimedOut),
        ));
        current.facts[0].origin = "documented sysfs attribute".into();
        current.retain_failed_values(&previous);
        let descriptor = current.telemetry.as_ref().unwrap();
        let data = super::super::assemble(&current, descriptor.kind, &descriptor.root).unwrap();
        assert_eq!(data.series[0].current.value, Some(44.0));
        assert_eq!(data.series[0].current.observed_ms, 1000);
        assert!(data.series[0].current.retained);
        assert_eq!(data.series[0].current.outcome, Outcome::TimedOut);
        current.generation = previous.generation + 1;
        current.telemetry = None;
        current.facts.clear();
        current.retain_failed_values(&previous);
        assert!(current.telemetry.is_none());
        assert!(current.facts.is_empty());
    }
    #[test]
    fn default_nvme_endpoint_limits_do_not_create_a_fabricated_operating_span() {
        let document = fixture(
            Family::Hwmon,
            &[
                ("name", "nvme"),
                ("temp2_input", "40000"),
                ("temp2_min", "-273150"),
                ("temp2_max", "65261850"),
            ],
        );
        let row = hwmon(&document, &document.path).2.remove(0);
        assert!(row.range.is_none());
        assert!(row.markers.iter().all(|point| point.value.is_none()));
        assert!(
            row.markers
                .iter()
                .any(|point| point.display.contains("65261.85"))
        );
        assert_eq!(
            named(&document, &document.path, "temp2_max")
                .unwrap()
                .value
                .as_deref(),
            Some("65261.85 °C")
        );
    }
    #[test]
    fn a_power_attribute_keeps_its_native_unit_instead_of_showing_a_health_ratio() {
        let mut document = fixture(
            Family::PowerSupply,
            &[
                ("type", "Battery"),
                ("energy_full", "51000000"),
                ("energy_full_design", "55000000"),
            ],
        );
        let root = document.path.clone();
        document.path = root.join("energy_full");
        let data = super::super::assemble(&document, super::super::Kind::Power, &root).unwrap();
        assert_eq!(data.series.len(), 1);
        assert_eq!(data.series[0].current.unit, "Wh");
        assert_eq!(data.series[0].current.value, Some(51.0));
        assert!(data.summary.contains("51 Wh"));
    }
    #[test]
    fn pwm_uses_the_documented_byte_scale_without_reinterpreting_cooling_states() {
        let document = fixture(Family::Hwmon, &[("name", "fixture"), ("pwm1", "128")]);
        let row = hwmon(&document, &document.path).2.remove(0);
        assert!((row.current.value.unwrap() - 50.1960784).abs() < 1e-6);
        assert_eq!(row.current.unit, "%");
        assert_eq!(row.range, Some((0.0, 100.0)));
    }
}

pub(super) fn brightness(
    document: &Document,
    root: &Path,
    backlight: bool,
) -> (String, String, Vec<Series>) {
    let name = root.file_name().unwrap_or_default().to_string_lossy();
    let role = if backlight {
        "Backlight brightness"
    } else {
        "LED brightness and trigger"
    };
    let mut summary = name.into_owned();
    let maximum = point(document, root, "max_brightness", "Maximum");
    let mut rows = Vec::new();
    for (attribute, label) in [
        (
            "brightness",
            if backlight { "Requested" } else { "Reported" },
        ),
        ("actual_brightness", "Actual"),
    ] {
        if attribute == "actual_brightness" && !backlight {
            continue;
        }
        let Some(mut current) = point(document, root, attribute, label) else {
            continue;
        };
        let mut notes = vec!["Brightness units are driver levels, not calibrated luminance; sequential observations are not an atomic snapshot".into()];
        let range = brightness_range(&mut current, maximum.as_ref(), &mut notes);
        let mut row = series(
            name_for_brightness(backlight, attribute).into(),
            current,
            maximum.clone().into_iter().collect(),
            notes,
            false,
        );
        row.range = range;
        summary.push_str(&format!(" · {label}: {}", row.current.display));
        rows.push(row);
    }
    if backlight {
        if let Some(kind) = text(document, root, "type") {
            summary.push_str(&format!(" · {kind}"));
        }
    } else if let Some(fact) = named(document, root, "trigger") {
        if let Some(Value::Selection {
            selected: Some(selected),
            ..
        }) = &fact.semantic
        {
            summary.push_str(&format!(" · Trigger: {selected}"));
        }
        if let Some(row) = rows.first_mut() {
            row.notes.push(format!(
                "Trigger/choices: {}",
                fact.value.as_deref().unwrap_or("unavailable")
            ));
        }
    }
    (role.into(), summary, rows)
}
fn name_for_brightness(backlight: bool, attribute: &str) -> &'static str {
    match (backlight, attribute) {
        (true, "actual_brightness") => "Hardware-reported brightness",
        (true, _) => "Requested brightness",
        (false, _) => "Kernel-reported LED level",
    }
}
fn brightness_range(
    current: &mut Point,
    maximum: Option<&Point>,
    notes: &mut Vec<String>,
) -> Option<(f64, f64)> {
    let max = maximum
        .and_then(|point| point.value)
        .filter(|value| value.is_finite() && *value > 0.0);
    let valid = current
        .value
        .zip(max)
        .filter(|(value, max)| value.is_finite() && *value >= 0.0 && *value <= *max);
    if let Some((value, max)) = valid {
        current.display = format!(
            "{:.1}% ({value:.0}/{max:.0}){}",
            value * 100.0 / max,
            if value == 0.0 {
                " · Off (reported)"
            } else {
                ""
            }
        );
        Some((0.0, max))
    } else {
        notes.push("Percentage unavailable: missing, zero or invalid maximum/brightness; raw observation retained".into());
        None
    }
}

pub(super) fn powercap(document: &Document, root: &Path) -> (String, String, Vec<Series>) {
    let name = text(document, root, "name")
        .map(str::to_string)
        .unwrap_or_else(|| {
            root.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
    let mut rows = Vec::new();
    for (field, label, max) in [
        (
            "energy_uj",
            "Energy counter snapshot",
            "max_energy_range_uj",
        ),
        ("power_uw", "Reported power", "max_power_range_uw"),
    ] {
        if let Some(current) = point(document, root, field, label) {
            let markers = point(document, root, max, "Exposed range")
                .into_iter()
                .collect();
            let mut row=series(label.into(),current,markers,vec!["Energy is a wrapping/resettable counter snapshot, not interval consumption; no power is inferred from a single counter observation".into()],false);
            row.range = row
                .current
                .value
                .zip(
                    row.markers
                        .first()
                        .filter(|marker| marker.unit == row.current.unit)
                        .and_then(|marker| marker.value),
                )
                .filter(|(value, max)| *value >= 0.0 && *max > 0.0 && *value <= *max)
                .map(|(_, max)| (0.0, max));
            rows.push(row);
        }
    }
    rows.extend(powercap_constraints(document, root));
    let mut summary = name;
    if let Some(value) = text(document, root, "enabled") {
        summary.push_str(&format!(" · Capping: {value}"));
    }
    let role = if rows.is_empty() && text(document, root, "name").is_none() {
        "Powercap control interface"
    } else {
        "Powercap energy and limits"
    };
    (role.into(), summary, rows)
}
fn powercap_constraints(document: &Document, root: &Path) -> Vec<Series> {
    let mut rows = Vec::new();
    let indexes = document
        .facts
        .iter()
        .filter(|fact| fact.source.parent() == Some(root))
        .filter_map(|fact| {
            fact.source
                .file_name()?
                .to_str()?
                .strip_prefix("constraint_")?
                .split_once('_')?
                .0
                .parse::<u32>()
                .ok()
        })
        .collect::<BTreeSet<_>>();
    for index in indexes {
        if let Some(current) = point(
            document,
            root,
            &format!("constraint_{index}_power_limit_uw"),
            "Configured limit",
        ) {
            let label = text(document, root, &format!("constraint_{index}_name"))
                .map(str::to_string)
                .unwrap_or_else(|| format!("Constraint {index}"));
            let markers = [("min_power_uw", "Minimum"), ("max_power_uw", "Maximum")]
                .iter()
                .filter_map(|(field, label)| {
                    point(
                        document,
                        root,
                        &format!("constraint_{index}_{field}"),
                        label,
                    )
                })
                .collect();
            let window = text(
                document,
                root,
                &format!("constraint_{index}_time_window_us"),
            )
            .unwrap_or("time window unavailable");
            rows.push(series(
                label,
                current,
                markers,
                vec![format!(
                    "Configured time window: {window}; limit setting is not measured power usage"
                )],
                false,
            ));
        }
    }
    rows
}
pub(super) fn regulator(document: &Document, root: &Path) -> (String, String, Vec<Series>) {
    let name = text(document, root, "name").unwrap_or("Regulator");
    let mut summary = name.to_string();
    for field in ["type", "state", "status", "opmode", "num_users"] {
        if let Some(value) = text(document, root, field) {
            summary.push_str(&format!(" · {field}: {value}"));
        }
    }
    let mut rows = Vec::new();
    for (field, label, min, max) in [
        (
            "microvolts",
            "Configured voltage",
            "min_microvolts",
            "max_microvolts",
        ),
        (
            "microamps",
            "Configured current limit",
            "min_microamps",
            "max_microamps",
        ),
        ("requested_microamps", "Consumer-requested load", "", ""),
    ] {
        if let Some(current) = point(document, root, field, label) {
            let markers = [min, max]
                .into_iter()
                .filter(|name| !name.trim().is_empty())
                .filter_map(|name| point(document, root, name, name))
                .collect();
            rows.push(series(label.into(),current,markers,vec!["Configured settings/requested load, not a physical voltage/current measurement; undefined constraints do not establish a numeric plot bound".into()],text(document,root,"status")==Some("error")));
        }
    }
    ("Regulator settings and constraints".into(), summary, rows)
}

pub(super) fn devfreq(document: &Document, root: &Path) -> (String, String, Vec<Series>) {
    let name = text(document, root, "name")
        .map(str::to_string)
        .unwrap_or_else(|| {
            root.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
    let governor = text(document, root, "governor").unwrap_or("governor unavailable");
    let summary = format!("{name} · {governor}");
    let mut rows = Vec::new();
    for (attribute, label) in [("cur_freq", "Reported"), ("target_freq", "Target/cached")] {
        if let Some(current) = point(document, root, attribute, label) {
            let mut notes=vec!["Reported frequency can use a cached fallback; this is not proof of measured clock frequency. Bounds reflect exposed kernel/QoS/OPP state, not an unconstrained hardware range".into()];
            let mut markers = [
                ("min_freq", "Exposed minimum"),
                ("max_freq", "Exposed maximum"),
            ]
            .iter()
            .filter_map(|(field, label)| point(document, root, field, label))
            .collect::<Vec<_>>();
            for marker in &mut markers {
                if marker.value == Some(0.0) {
                    marker.value = None;
                    notes.push(format!("{}: zero excluded as an operating bound (legacy ABI permits no user restriction)",marker.label));
                }
            }
            rows.push(series(
                format!("{name} · {label}"),
                current,
                markers,
                notes,
                false,
            ));
        }
    }
    ("Device frequency scaling".into(), summary, rows)
}
