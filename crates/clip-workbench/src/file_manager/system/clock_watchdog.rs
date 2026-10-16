//! RTC observations and watchdog metadata; access devices are never opened.
use super::{
    Document, Relation,
    attributes::{self, Rule},
    buses::{self, Topology},
    collector::Reads,
    model::Fact,
};
use std::{io, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Object {
    Rtc,
    Watchdog,
}

fn numbered(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix).is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix.bytes().all(|byte| byte.is_ascii_digit())
            && suffix.parse::<u32>().is_ok()
    })
}
fn object(path: &Path) -> Option<Object> {
    let name = path.file_name()?.to_str()?;
    match buses::subsystem(path).as_deref()? {
        "rtc" if numbered(name, "rtc") => Some(Object::Rtc),
        "watchdog" if numbered(name, "watchdog") => Some(Object::Watchdog),
        _ => None,
    }
}
pub(super) fn rules(path: &Path) -> Vec<Rule> {
    let specs = match object(path) {
        Some(Object::Rtc) => attributes::RTC,
        Some(Object::Watchdog) => attributes::WATCHDOG,
        None => &[],
    };
    super::attributes::materialize_specs(specs)
}

pub(super) fn collect(
    document: &mut Document,
    sys: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    collect_at(
        document,
        sys,
        Path::new("/sys/class/watchdog/watchdog0"),
        reads,
        checkpoint,
    );
}
fn collect_at(
    document: &mut Document,
    sys: &Path,
    legacy: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let mapped;
    let root = if object(sys).is_some() {
        sys
    } else if matches!(
        document.kind,
        little_clip_core::kernel_interface::Kind::Character {
            major: 10,
            minor: 130
        }
    ) {
        mapped = legacy.canonicalize().ok();
        let Some(root) = mapped
            .as_deref()
            .filter(|root| object(root) == Some(Object::Watchdog))
        else {
            legacy_policy(document);
            return;
        };
        document.relations.push(Relation::new(
            "Legacy watchdog core registration",
            root.into(),
        ));
        root
    } else {
        return;
    };
    let Some(selected) = object(root) else {
        return;
    };
    for rule in rules(root) {
        reads.observe(document, root, &rule);
        checkpoint(document);
    }
    document.title = match selected {
        Object::Rtc => "RTC clock metadata",
        Object::Watchdog => "Watchdog metadata (device unopened)",
    }
    .into();
    let mut topology = Topology::new(
        "Clock/watchdog relationships",
        root.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
    );
    let ancestors = buses::ancestors(root, 12, |path| {
        matches!(
            buses::subsystem(path).as_deref(),
            Some("platform" | "pci" | "i2c" | "spi" | "usb")
        )
    });
    let parent = buses::ancestor_chain(&mut topology, document, ancestors, None);
    let label = root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let anchor = buses::add(&mut topology, document, root.into(), label, parent);
    buses::access_node(document, root, "Clock/watchdog access interface");
    for name in ["device", "firmware_node", "of_node", "driver"] {
        if let Ok(path) = root.join(name).canonicalize() {
            document
                .relations
                .push(Relation::new(format!("Clock/watchdog {name}"), path));
        }
    }
    let driver = root
        .ancestors()
        .take(12)
        .find_map(|path| buses::driver(path).map(|driver| (path, driver)));
    if let Some((path, name)) = driver {
        topology.summary.push_str(&format!(" · {name}"));
        document.facts.push(Fact::text(
            "Observed clock/watchdog driver",
            name,
            path.join("driver"),
        ));
    }
    match selected {
        Object::Rtc => rtc(document, root, &mut topology, anchor),
        Object::Watchdog => watchdog(document, root, &mut topology),
    }
    document.topology = Some(topology);
    document.facts.sort_by_key(|fact| fact.interface);
    checkpoint(document);
}

fn rtc(document: &mut Document, root: &Path, topology: &mut Topology, anchor: usize) {
    if let Some(name) = document.observed_raw("Clock identity") {
        topology.summary.push_str(&format!(" · {name}"));
    }
    match document
        .observed_fact("RTC seconds since epoch")
        .and_then(|source| source.raw.as_deref().map(|raw| (source, calendar(raw))))
    {
        Some((source, Ok(value))) => {
            topology
                .summary
                .push_str(&format!(" · {value} (timezone unspecified)"));
            let mut fact = Fact::text(
                "RTC calendar snapshot (timezone unspecified)",
                value,
                root.join("since_epoch"),
            );
            fact.raw = source.raw.clone();
            fact.inherit_observation(source);
            fact.origin = "derived calendar from observed RTC epoch".into();
            document.facts.push(fact);
        }
        Some((source, Err(error))) => {
            let mut fact = Fact::failure("RTC calendar snapshot", root.join("since_epoch"), error);
            fact.inherit_observation(source);
            document.facts.push(fact);
        }
        None => {
            for label in ["RTC date", "RTC time (timezone unspecified)"] {
                if let Some(value) = document.observed_raw(label) {
                    topology.summary.push_str(&format!(" · {value}"));
                }
            }
        }
    }
    match std::fs::metadata(root.join("wakealarm")) {
        Ok(_) => {
            document.facts.push(Fact::text(
                "RTC wake-alarm interface",
                "Exposed; alarm configuration not changed",
                root.join("wakealarm"),
            ));
            if let Some((source, value)) = document
                .observed_fact("Wake alarm")
                .and_then(|source| source.raw.as_deref().map(|raw| (source, raw)))
            {
                let summary = if value.is_empty() {
                    "No programmed alarm reported".into()
                } else {
                    format!("Programmed RTC alarm: {value} seconds since epoch")
                };
                let mut fact = Fact::text(
                    "RTC wake-alarm observation",
                    summary,
                    root.join("wakealarm"),
                );
                fact.inherit_observation(source);
                document.facts.push(fact);
            }
        }
        Err(error) => document.facts.push(Fact::failure(
            "RTC wake-alarm interface",
            root.join("wakealarm"),
            error,
        )),
    }
    if root.join("alarmtimer.0.auto").is_dir() {
        buses::add(
            topology,
            document,
            root.join("alarmtimer.0.auto"),
            "Associated alarmtimer".into(),
            Some(anchor),
        );
    }
    document.facts.push(Fact::text("RTC observation policy","RTC calendar/timezone is not inferred from system timezone; observations can differ from the system clock; no device/interrupt stream opened",root.into()));
}
fn calendar(raw: &str) -> io::Result<String> {
    let timestamp = raw.parse::<i64>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "RTC epoch is outside signed 64-bit range",
        )
    })?;
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|time| time.format("%Y-%m-%d %H:%M:%S").to_string())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "RTC calendar is outside the supported date range",
            )
        })
}
fn watchdog(document: &mut Document, root: &Path, topology: &mut Topology) {
    for (label, prefix) in [
        ("Watchdog identity", ""),
        ("Watchdog timeout (s)", "timeout "),
        ("Watchdog time left (s)", "time left "),
        ("Watchdog kernel active state", "kernel state "),
    ] {
        if let Some(value) = document.observed_raw(label) {
            topology.summary.push_str(&format!(" · {prefix}{value}"));
            if label.ends_with("(s)") {
                topology.summary.push_str(" s");
            }
        }
    }
    document.facts.push(Fact::text("Watchdog observation policy","Sysfs metadata only; device not opened, started, pinged, serviced or stopped. Kernel inactive state is not proof that hardware is stopped",root.into()));
}
fn legacy_policy(document: &mut Document) {
    document.title = "Legacy watchdog interface (registration unavailable)".into();
    let mut fact = Fact::text(
        "Watchdog observation policy",
        "Legacy major/minor identity; core sysfs registration not exposed; device not opened or serviced",
        document.path.clone(),
    );
    fact.interface = true;
    document.facts.insert(0, fact);
}
pub(super) fn unmapped(document: &mut Document) {
    if matches!(
        document.kind,
        little_clip_core::kernel_interface::Kind::Character {
            major: 10,
            minor: 130
        }
    ) && document.topology.is_none()
    {
        legacy_policy(document);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, os::unix::fs::symlink};

    #[test]
    fn rtc_derived_calendar_and_alarm_preserve_source_age_and_known_or_unknown_generation() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("wakealarm"), "").unwrap();
        for generation in [Some(7), None] {
            let mut doc = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
            doc.facts.clear();
            doc.generation = 99;
            for (label, name, raw, observed_ms) in [
                ("RTC seconds since epoch", "since_epoch", "0", 123),
                ("Wake alarm", "wakealarm", "", 456),
            ] {
                let mut source = Fact::text(label, raw, root.join(name));
                source.raw = Some(raw.into());
                source.observed_ms = observed_ms;
                source.observed_generation = generation;
                let mut encoded = serde_json::to_value(source).unwrap();
                encoded
                    .as_object_mut()
                    .unwrap()
                    .remove("generation_pending");
                doc.facts.push(serde_json::from_value(encoded).unwrap());
            }
            let mut topology = Topology::new("RTC", String::new());
            rtc(&mut doc, root, &mut topology, 0);
            doc.stamp_fact_generations();
            for (label, name, observed_ms) in [
                (
                    "RTC calendar snapshot (timezone unspecified)",
                    "since_epoch",
                    123,
                ),
                ("RTC wake-alarm observation", "wakealarm", 456),
            ] {
                let derived = doc.facts.iter().find(|fact| fact.label == label).unwrap();
                assert_eq!(derived.observed_ms, observed_ms);
                assert_eq!(derived.observed_generation, generation);
                assert_eq!(derived.source, root.join(name));
            }
            doc.facts
                .iter_mut()
                .find(|fact| fact.label == "RTC seconds since epoch")
                .unwrap()
                .raw = Some("9223372036854775808".into());
            rtc(&mut doc, root, &mut topology, 0);
            doc.stamp_fact_generations();
            let invalid = doc
                .facts
                .iter()
                .find(|fact| fact.label == "RTC calendar snapshot")
                .unwrap();
            assert_eq!(invalid.outcome, super::super::model::Outcome::Malformed);
            assert_eq!(invalid.observed_ms, 123);
            assert_eq!(invalid.observed_generation, generation);
        }
    }
    #[test]
    fn rtc_calendar_and_wake_alarm_do_not_infer_utc_or_modify_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("rtc0");
        let class = temp.path().join("class/rtc");
        std::fs::create_dir_all(&class).unwrap();
        std::fs::create_dir(&root).unwrap();
        symlink(&class, root.join("subsystem")).unwrap();
        std::fs::write(root.join("wakealarm"), b"").unwrap();
        let mut reads = Reads::fixture(BTreeMap::from([
            (root.join("name"), b"example-rtc".to_vec()),
            (root.join("since_epoch"), b"0".to_vec()),
            (root.join("wakealarm"), b"".to_vec()),
            (root.join("hctosys"), b"1".to_vec()),
        ]));
        let mut doc = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        collect(&mut doc, &root, &mut reads, &mut |_| {});
        assert!(
            doc.topology
                .as_ref()
                .unwrap()
                .summary
                .contains("1970-01-01 00:00:00 (timezone unspecified)")
        );
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "RTC wake-alarm observation"
                    && fact.value.as_deref() == Some("No programmed alarm reported"))
        );
        assert!(doc.facts.iter().any(|fact| fact.label
            == "RTC calendar snapshot (timezone unspecified)"
            && fact.raw.as_deref() == Some("0")));
        assert!(
            reads
                .attempts()
                .all(|path| path != root.join("max_user_freq"))
        );
        let nested = root.join("private");
        std::fs::create_dir(&nested).unwrap();
        assert!(rules(&nested).is_empty());
    }

    #[test]
    fn watchdog_legacy_mapping_uses_sysfs_without_device_content_or_keepalive() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("watchdog0");
        let class = temp.path().join("class/watchdog");
        let misc = temp.path().join("misc/watchdog");
        std::fs::create_dir_all(&class).unwrap();
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir_all(&misc).unwrap();
        symlink(&class, root.join("subsystem")).unwrap();
        let mut reads = Reads::fixture(BTreeMap::from([
            (root.join("identity"), b"example-wdt".to_vec()),
            (root.join("timeout"), b"30".to_vec()),
            (root.join("timeleft"), b"8".to_vec()),
            (root.join("state"), b"inactive".to_vec()),
            (root.join("nowayout"), b"1".to_vec()),
        ]));
        let mut doc = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        doc.kind = little_clip_core::kernel_interface::Kind::Character {
            major: 10,
            minor: 130,
        };
        collect_at(&mut doc, &misc, &root, &mut reads, &mut |_| {});
        assert_eq!(doc.title, "Watchdog metadata (device unopened)");
        assert!(
            doc.topology
                .as_ref()
                .unwrap()
                .summary
                .contains("timeout 30 s")
        );
        assert!(doc.relations.iter().any(|relation| relation.target == root));
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "Watchdog nowayout"
                    && fact.value.as_deref() == Some("Set"))
        );
        assert!(reads.attempts().all(|path| path.starts_with(&root)));
        assert!(reads.attempts().all(|path| !matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("access_cs0" | "keepalive" | "pretimeout_governor")
        )));
        assert!(doc.facts.iter().any(|fact| {
            fact.label == "Watchdog observation policy"
                && fact
                    .value
                    .as_ref()
                    .is_some_and(|value| value.contains("inactive state is not proof"))
        }));
    }

    #[test]
    fn missing_watchdog_registration_and_malformed_time_data_remain_explicit() {
        let temp = tempfile::tempdir().unwrap();
        let mut reads = Reads::fixture(BTreeMap::new());
        let mut doc = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        doc.kind = little_clip_core::kernel_interface::Kind::Character {
            major: 10,
            minor: 130,
        };
        collect_at(
            &mut doc,
            temp.path(),
            &temp.path().join("missing-watchdog0"),
            &mut reads,
            &mut |_| {},
        );
        assert!(doc.title.contains("registration unavailable"));
        assert_eq!(reads.attempts().count(), 0);
        assert_eq!(calendar("-1").unwrap(), "1969-12-31 23:59:59");
        for raw in [
            "not a number",
            "9223372036854775807",
            "18446744073709551615",
        ] {
            assert!(calendar(raw).is_err());
        }
        assert!(!numbered("rtc-1", "rtc"));
        assert!(!numbered("watchdog4294967296", "watchdog"));
        assert!(rules(Path::new("/unregistered/watchdog0")).is_empty());
    }
}
