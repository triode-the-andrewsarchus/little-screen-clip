//! Powercap counters/constraints and regulator settings; getters only.
use super::{
    Document, Relation,
    attributes::{self, Format, Rule},
    buses,
    model::{Fact, Outcome},
};
use std::{collections::BTreeSet, path::Path};
fn role(path: &Path) -> Option<bool> {
    match buses::subsystem(path).as_deref()? {
        "powercap" => Some(true),
        "regulator"
            if path
                .file_name()?
                .to_str()?
                .strip_prefix("regulator.")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) =>
        {
            Some(false)
        }
        _ => None,
    }
}
fn zone(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| {
            let mut parts = name.split(':');
            parts.next().is_some_and(|p| !p.is_empty())
                && parts.clone().next().is_some()
                && parts.all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        })
}
fn constraint(name: &str) -> Option<(u32, &str)> {
    let (index, field) = name.strip_prefix("constraint_")?.split_once('_')?;
    if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((index.parse().ok()?, field))
}
pub(super) fn constraint_rule(path: &Path, name: &str) -> Option<Rule> {
    if role(path) != Some(true) || !zone(path) {
        return None;
    }
    let (index, field) = constraint(name)?;
    let (label, format) = constraint_field(field)?;
    Some(Rule::new(
        name,
        &format!("Constraint {index} {label}"),
        format,
    ))
}
fn constraint_field(field: &str) -> Option<(&'static str, Format)> {
    Some(match field {
        "name" => ("Name", Format::Text),
        "power_limit_uw" => (
            "Configured power limit",
            Format::UnsignedScale(0.000001, "W"),
        ),
        "min_power_uw" => (
            "Minimum allowed power limit",
            Format::UnsignedScale(0.000001, "W"),
        ),
        "max_power_uw" => (
            "Maximum allowed power limit",
            Format::UnsignedScale(0.000001, "W"),
        ),
        "time_window_us" => (
            "Configured time window",
            Format::UnsignedScale(0.000001, "s"),
        ),
        "min_time_window_us" => ("Minimum time window", Format::UnsignedScale(0.000001, "s")),
        "max_time_window_us" => ("Maximum time window", Format::UnsignedScale(0.000001, "s")),
        _ => return None,
    })
}
fn indexes(path: &Path) -> (Vec<u32>, Outcome) {
    let Ok(entries) = std::fs::read_dir(path) else {
        return (Vec::new(), Outcome::Unsupported);
    };
    let (paths, mut coverage) = buses::select_children(
        entries,
        |entry| {
            entry
                .file_name()
                .to_str()
                .and_then(constraint)
                .is_some_and(|(_, field)| constraint_field(field).is_some())
        },
        64,
    );
    let mut ids = paths
        .iter()
        .filter_map(|path| constraint(path.file_name()?.to_str()?).map(|(id, _)| id))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if ids.len() > 8 {
        ids.truncate(8);
        coverage = Outcome::Truncated;
    }
    (ids, coverage)
}
pub(super) fn rules(path: &Path) -> Vec<Rule> {
    use Format::{OptionalScale as O, Scale as S, Text as T, Unsigned as U, UnsignedScale as E};
    let specs: &[(&str, &str, Format)] = match role(path) {
        Some(true) if zone(path) => &[
            ("name", "Energy domain", T),
            (
                "enabled",
                "Power capping state",
                Format::Boolean("Enabled", "Disabled"),
            ),
            ("energy_uj", "Energy counter", E(0.000001, "J")),
            (
                "max_energy_range_uj",
                "Energy counter range",
                E(0.000001, "J"),
            ),
            ("power_uw", "Reported power", E(0.000001, "W")),
            (
                "max_power_range_uw",
                "Reported power range",
                E(0.000001, "W"),
            ),
        ],
        Some(true) => &[(
            "enabled",
            "Powercap control state",
            Format::Boolean("Enabled", "Disabled"),
        )],
        Some(false) => &[
            ("name", "Regulator name", T),
            ("type", "Regulator type", T),
            ("state", "Regulator enable control", T),
            ("status", "Regulator output status", T),
            ("opmode", "Configured operating mode", T),
            ("microvolts", "Configured voltage", S(0.000001, "V")),
            (
                "min_microvolts",
                "Minimum voltage constraint",
                O(0.000001, "V"),
            ),
            (
                "max_microvolts",
                "Maximum voltage constraint",
                O(0.000001, "V"),
            ),
            ("microamps", "Configured current limit", S(0.000001, "A")),
            (
                "min_microamps",
                "Minimum current constraint",
                O(0.000001, "A"),
            ),
            (
                "max_microamps",
                "Maximum current constraint",
                O(0.000001, "A"),
            ),
            (
                "requested_microamps",
                "Consumer-requested load",
                S(0.000001, "A"),
            ),
            ("num_users", "Kernel enable-user count", U),
        ],
        None => &[],
    };
    let mut rules = attributes::materialize_specs(specs);
    if role(path) == Some(true) && zone(path) {
        for index in indexes(path).0 {
            for field in [
                "name",
                "power_limit_uw",
                "min_power_uw",
                "max_power_uw",
                "time_window_us",
                "min_time_window_us",
                "max_time_window_us",
            ] {
                if let Some(rule) = constraint_rule(path, &format!("constraint_{index}_{field}")) {
                    rules.push(rule);
                }
            }
        }
    }
    rules
}
pub(super) fn collect(document: &mut Document, path: &Path, checkpoint: &mut dyn FnMut(&Document)) {
    let Some(powercap) = role(path) else {
        return;
    };
    if powercap && zone(path) {
        let (_, coverage) = indexes(path);
        let mut fact = Fact::text(
            "Powercap constraint inventory coverage",
            coverage.to_string(),
            path.into(),
        );
        fact.outcome = coverage;
        document.facts.push(fact);
    }
    buses::link_relations(
        document,
        path,
        &["parent", "device", "driver", "firmware_node", "of_node"],
        "Power domain",
    );
    if powercap {
        for ancestor in buses::ancestors(path, 12, |parent| role(parent) == Some(true)) {
            document
                .relations
                .push(Relation::new("Parent powercap domain/control", ancestor));
        }
        if let Ok(entries) = std::fs::read_dir(path) {
            let (paths, coverage) = buses::select_children(
                entries,
                |entry| {
                    entry.file_type().is_ok_and(|kind| kind.is_dir())
                        && role(&entry.path()) == Some(true)
                },
                16,
            );
            let mut fact = Fact::text(
                "Child powercap domain coverage",
                coverage.to_string(),
                path.into(),
            );
            fact.outcome = coverage;
            document.facts.push(fact);
            for child in paths {
                document
                    .relations
                    .push(Relation::new("Child powercap domain", child));
            }
        }
    }
    let policy = if powercap {
        "Energy is a counter snapshot, not interval consumption or inferred watts; counters can wrap/reset. Configured limits are not measured usage; no counter reset or capping changes"
    } else {
        "Voltage/current/mode are configured settings, not proof of actual output; enable control and output status remain separate. Documented getters may query hardware; no regulator control changes"
    };
    document.facts.push(Fact::text(
        "Power domain observation policy",
        policy,
        path.into(),
    ));
    checkpoint(document);
}

#[cfg(test)]
mod tests {
    use super::super::{attributes::Context, telemetry};
    use super::*;
    use std::{os::unix::fs::symlink, path::PathBuf};
    fn fixture(
        subsystem: &str,
        name: &str,
        values: &[(&str, &str)],
    ) -> (tempfile::TempDir, PathBuf, Document) {
        let (temp, root, mut reads, mut doc) =
            super::super::collector::registered_fixture(subsystem, name, values);
        telemetry::collect(&mut doc, &root, &Context::discover(&root), &mut reads);
        collect(&mut doc, &root, &mut |_| {});
        (temp, root, doc)
    }
    #[test]
    fn energy_counter_and_constraints_keep_units_without_inferred_instantaneous_power() {
        let (_temp, _, doc) = fixture(
            "powercap",
            "test-cap:0",
            &[
                ("name", "Package"),
                ("enabled", "0"),
                ("energy_uj", "5000000"),
                ("max_energy_range_uj", "10000000"),
                ("constraint_0_name", "long-term"),
                ("constraint_0_power_limit_uw", "15000000"),
                ("constraint_0_min_power_uw", "10000000"),
                ("constraint_0_max_power_uw", "20000000"),
                ("constraint_0_time_window_us", "100000"),
            ],
        );
        let data = doc.telemetry.as_ref().unwrap();
        assert!(data.summary.contains("Capping: Disabled"));
        assert_eq!(data.series.len(), 2);
        assert_eq!(data.series[0].current.unit, "J");
        assert_eq!(data.series[0].current.value, Some(5.0));
        assert_eq!(data.series[0].range, Some((0.0, 10.0)));
        assert_eq!(data.series[1].current.unit, "W");
        assert_eq!(data.series[1].current.value, Some(15.0));
        assert!(data.series[1].notes[0].contains("0.1 s"));
        assert!(doc.observed_raw("Reported power").is_none());
    }
    #[test]
    fn regulator_setting_is_not_measured_output_and_undefined_constraints_stay_nonnumeric() {
        let (_temp, _, doc) = fixture(
            "regulator",
            "regulator.0",
            &[
                ("name", "Rail"),
                ("type", "voltage"),
                ("state", "disabled"),
                ("status", "off"),
                ("microvolts", "3300000"),
                ("min_microvolts", "constraint not defined"),
                ("max_microvolts", "3600000"),
                ("requested_microamps", "1000"),
                ("num_users", "2"),
            ],
        );
        let data = doc.telemetry.as_ref().unwrap();
        assert!(data.summary.contains("disabled"));
        assert_eq!(data.series[0].current.value, Some(3.3));
        assert_eq!(data.series[0].current.unit, "V");
        assert!(
            data.series[0]
                .markers
                .iter()
                .any(|marker| marker.display == "constraint not defined" && marker.value.is_none())
        );
        assert!(!data.series[0].alarm);
        assert!(doc.facts.iter().any(|fact| {
            fact.label == "Power domain observation policy"
                && fact
                    .value
                    .as_ref()
                    .unwrap()
                    .contains("not proof of actual output")
        }));
    }
    #[test]
    fn unsigned_energy_preserves_large_raw_values_and_rejects_negative_counters() {
        let (_temp, root, doc) = fixture(
            "powercap",
            "test-cap:0",
            &[("energy_uj", "18446744073709551615")],
        );
        assert_eq!(
            doc.observed_raw("Energy counter"),
            Some("18446744073709551615")
        );
        let rule = rules(&root)
            .into_iter()
            .find(|rule| rule.name == "energy_uj")
            .unwrap();
        assert!(attributes::parse(&rule, "-1").is_err());
        let (_temp, _, doc) = fixture("powercap", "test-cap:0", &[("energy_uj", "-1")]);
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "Energy counter" && fact.outcome == Outcome::Malformed)
        );
    }
    #[test]
    fn bounded_constraint_inventory_does_not_block_an_explicit_high_index_attribute() {
        let (temp, root, _) = fixture("powercap", "test-cap:0", &[]);
        for index in 0..20 {
            std::fs::write(root.join(format!("constraint_{index}_name")), "limit").unwrap();
        }
        let (indexes, coverage) = indexes(&root);
        assert_eq!(indexes.len(), 8);
        assert_eq!(coverage, Outcome::Truncated);
        assert!(constraint_rule(&root, "constraint_99_power_limit_uw").is_some());
        assert!(constraint_rule(&root, "constraint_99_vendor_register").is_none());
        assert!(constraint_rule(&root, "constraint_-1_name").is_none());
        let nested = root.join("private");
        std::fs::create_dir(&nested).unwrap();
        assert!(rules(&nested).is_empty());
        let controller = temp.path().join("test-cap");
        std::fs::create_dir(&controller).unwrap();
        symlink(
            temp.path().join("class/powercap"),
            controller.join("subsystem"),
        )
        .unwrap();
        assert!(rules(&controller).iter().all(|rule| rule.name == "enabled"));
    }
}
