//! Radio blocking and device-frequency observations, without control writes or streams.
use super::{
    Document,
    attributes::{self, Format, Rule},
    buses::{self, Topology},
    collector::Reads,
    model::Fact,
};
use std::path::Path;
fn radio(path: &Path) -> bool {
    buses::subsystem(path).as_deref() == Some("rfkill")
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_prefix("rfkill"))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}
fn frequency(path: &Path) -> bool {
    buses::subsystem(path).as_deref() == Some("devfreq")
}
pub(super) fn rules(path: &Path) -> Vec<Rule> {
    use Format::{Boolean as B, FrequencyHz as H, Text as T};
    let specs: &[(&str, &str, Format)] = if radio(path) {
        &[
            ("name", "Radio identity", T),
            ("type", "Radio type", T),
            ("soft", "Software block", B("Blocked", "Unblocked")),
            ("hard", "Hardware block", B("Blocked", "Unblocked")),
            (
                "persistent",
                "Persistent software-block initialization",
                B("Yes", "No"),
            ),
        ]
    } else if frequency(path) {
        &[
            ("name", "Device-frequency identity", T),
            ("governor", "Frequency governor", T),
            (
                "available_governors",
                "Available governors",
                Format::Selection,
            ),
            ("cur_freq", "Reported device frequency", H),
            ("target_freq", "Cached/governor target frequency", H),
            ("min_freq", "Exposed minimum frequency", H),
            ("max_freq", "Exposed maximum frequency", H),
            ("available_frequencies", "Available frequencies (Hz)", T),
            ("related_cpus", "Related CPUs", Format::CpuList),
        ]
    } else {
        &[]
    };
    attributes::materialize_specs(specs)
}
pub(super) fn collect(
    document: &mut Document,
    root: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    if !radio(root) && !frequency(root) {
        access(document, root, reads, checkpoint);
        return;
    }
    for rule in rules(root) {
        reads.observe(document, root, &rule);
        checkpoint(document);
    }
    buses::link_relations(
        document,
        root,
        &["device", "driver", "firmware_node", "of_node"],
        "Radio/frequency",
    );
    if radio(root) {
        document.title = "Radio block state".into();
        let topology = radio_topology(document, root);
        document.topology = Some(topology);
        document.facts.push(Fact::text("Radio observation policy","Software/hardware block observations are separate; unblocked is not proof the transmitter is active or connected. No radio enable/block changes or rfkill event-stream reads",root.into()));
    } else {
        document.facts.push(Fact::text("Device-frequency observation policy","Reported frequency may use the driver's cached fallback; target/bounds are not proof of measured clock frequency. Exposed bounds can reflect QoS/OPPs; zero is not a useful operating bound. No governor, frequency, polling or statistics changes",root.into()));
    }
    checkpoint(document);
}
fn radio_topology(document: &mut Document, root: &Path) -> Topology {
    let name = document
        .observed_attribute(root, "name")
        .and_then(|fact| fact.value.clone())
        .unwrap_or_else(|| {
            root.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
    let mut summary = name.clone();
    for (attribute, label) in [("type", "Type"), ("soft", "Software"), ("hard", "Hardware")] {
        let value = document
            .observed_attribute(root, attribute)
            .and_then(|fact| fact.value.as_deref())
            .unwrap_or("unavailable");
        summary.push_str(&format!(" · {label}: {value}"));
    }
    let mut topology = Topology::new("Radio hardware relationships", summary);
    let parents = buses::ancestors(root, 12, |path| {
        matches!(
            buses::subsystem(path).as_deref(),
            Some("usb" | "pci" | "platform" | "bluetooth" | "ieee80211" | "sdio")
        )
    });
    let parent = buses::ancestor_chain(&mut topology, document, parents, None);
    buses::add(&mut topology, document, root.into(), name, parent);
    topology
}

fn access(
    document: &mut Document,
    root: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    access_at(
        document,
        root,
        Path::new("/sys/class/misc/rfkill"),
        Path::new("/sys/class/rfkill"),
        reads,
        checkpoint,
    );
}
fn access_at(
    document: &mut Document,
    root: &Path,
    reference: &Path,
    class: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    if !super::boot_firmware::same_object(root, reference) {
        return;
    }
    document.title = "RFkill access stream (unopened)".into();
    let mut topology = Topology::new(
        "Registered radio block controls",
        "RFkill event/control interface; stream unopened".into(),
    );
    match std::fs::read_dir(class) {
        Ok(entries) => {
            let (paths, coverage) =
                buses::select_children(entries, |entry| radio(&entry.path()), 8);
            topology.coverage = coverage;
            for path in paths {
                for rule in rules(&path) {
                    reads.observe(document, &path, &rule);
                }
                let label = radio_topology(document, &path).summary;
                buses::add(&mut topology, document, path, label, None);
                checkpoint(document);
            }
        }
        Err(error) => {
            let fact = Fact::failure("Radio registration inventory", class.into(), error);
            topology.coverage = fact.outcome.clone();
            document.facts.push(fact);
        }
    }
    document.topology = Some(topology);
    document.facts.push(Fact::text("RFkill access policy","Registered sysfs metadata only; event/control device unopened; no radio blocking, unblocking or stream consumption",root.into()));
    checkpoint(document);
}

#[cfg(test)]
mod tests {
    use super::super::collector::registered_fixture as fixture;
    use super::super::{
        attributes::Context,
        model::{Outcome, Value},
        telemetry,
    };
    use super::*;
    use std::{collections::BTreeMap, os::unix::fs::symlink};
    #[test]
    fn radio_soft_and_hard_blocks_are_independent_and_unblocked_is_not_active() {
        let (_temp, root, mut reads, mut doc) = fixture(
            "rfkill",
            "rfkill0",
            &[
                ("name", "Radio"),
                ("type", "bluetooth"),
                ("soft", "1"),
                ("hard", "1"),
            ],
        );
        collect(&mut doc, &root, &mut reads, &mut |_| {});
        let data = doc.topology.as_ref().unwrap();
        assert!(data.summary.contains("Software: Blocked"));
        assert!(data.summary.contains("Hardware: Blocked"));
        assert!(doc.facts.iter().any(|fact| {
            fact.label == "Radio observation policy"
                && fact
                    .value
                    .as_ref()
                    .unwrap()
                    .contains("not proof the transmitter is active")
        }));
        assert!(reads.attempts().all(|path| !matches!(
            path.file_name().and_then(|n| n.to_str()),
            Some("state" | "claim")
        )));
        let (_temp, root, mut reads, mut doc) =
            fixture("rfkill", "rfkill1", &[("soft", "0"), ("hard", "invalid")]);
        collect(&mut doc, &root, &mut reads, &mut |_| {});
        assert!(
            doc.topology
                .as_ref()
                .unwrap()
                .summary
                .contains("Hardware: unavailable")
        );
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "Hardware block" && fact.outcome == Outcome::Malformed)
        );
    }
    #[test]
    fn radio_summary_keeps_historical_block_values_out_of_current_state() {
        let (_temp, root, mut reads, mut doc) = fixture(
            "rfkill",
            "rfkill3",
            &[
                ("name", "Cached radio"),
                ("type", "wlan"),
                ("soft", "1"),
                ("hard", "0"),
            ],
        );
        collect(&mut doc, &root, &mut reads, &mut |_| {});
        for fact in &mut doc.facts {
            if fact.source == root.join("name") || fact.source == root.join("soft") {
                fact.retained = true;
            }
            if fact.source == root.join("hard") {
                fact.outcome = Outcome::PermissionDenied;
            }
        }
        let topology = radio_topology(&mut doc, &root);
        assert!(topology.summary.starts_with("rfkill3"));
        assert!(topology.summary.contains("Type: wlan"));
        assert!(topology.summary.contains("Software: unavailable"));
        assert!(topology.summary.contains("Hardware: unavailable"));
        assert!(!topology.summary.contains("Cached radio"));
        assert!(!topology.summary.contains("Blocked"));
        assert!(!topology.summary.contains("Unblocked"));
        assert!(doc.facts.iter().any(|fact| fact.source == root.join("soft")
            && fact.retained
            && fact.value.as_deref() == Some("Blocked")));
    }
    #[test]
    fn devfreq_uses_hz_preserves_reported_and_target_values_and_excludes_zero_bounds() {
        let (_temp, root, mut reads, mut doc) = fixture(
            "devfreq",
            "memory-controller",
            &[
                ("name", "Memory controller"),
                ("governor", "simple_ondemand"),
                ("cur_freq", "200000000"),
                ("target_freq", "300000000"),
                ("min_freq", "0"),
                ("max_freq", "400000000"),
            ],
        );
        telemetry::collect(&mut doc, &root, &Context::discover(&root), &mut reads);
        collect(&mut doc, &root, &mut reads, &mut |_| {});
        let data = doc.telemetry.as_ref().unwrap();
        assert_eq!(data.series.len(), 2);
        assert_eq!(data.series[0].current.value, Some(200000000.0));
        assert_eq!(data.series[0].current.display, "200 MHz");
        assert_eq!(data.series[1].current.display, "300 MHz");
        assert!(
            data.series[0]
                .markers
                .iter()
                .any(|marker| marker.display == "0 Hz" && marker.value.is_none())
        );
        assert!(data.series[0].notes[0].contains("cached fallback"));
        assert!(reads.attempts().all(|path| !matches!(
            path.file_name().and_then(|n| n.to_str()),
            Some("trans_stat" | "polling_interval" | "timer" | "set_freq")
        )));
        let rule = rules(&root)
            .into_iter()
            .find(|rule| rule.name == "cur_freq")
            .unwrap();
        let (value, display) = attributes::parse(&rule, "1").unwrap();
        assert_eq!(display, "1 Hz");
        assert_eq!(
            value,
            Value::Measurement {
                value: 1.0,
                unit: "Hz".into()
            }
        );
    }
    #[test]
    fn access_interface_uses_registration_metadata_without_opening_a_radio_stream() {
        let (temp, root, _, mut doc) = fixture(
            "rfkill",
            "rfkill0",
            &[("name", "Radio"), ("soft", "0"), ("hard", "0")],
        );
        let class = temp.path().join("registered");
        std::fs::create_dir(&class).unwrap();
        symlink(&root, class.join("rfkill0")).unwrap();
        // Fixture reads use the class alias passed to the collector, matching real syspath rules.
        let mut reads = Reads::fixture(BTreeMap::from([
            (class.join("rfkill0/name"), b"Radio".to_vec()),
            (class.join("rfkill0/soft"), b"0".to_vec()),
            (class.join("rfkill0/hard"), b"0".to_vec()),
        ]));
        let access = temp.path().join("misc-rfkill");
        std::fs::create_dir(&access).unwrap();
        access_at(&mut doc, &access, &access, &class, &mut reads, &mut |_| {});
        assert_eq!(doc.title, "RFkill access stream (unopened)");
        assert_eq!(doc.topology.as_ref().unwrap().nodes.len(), 1);
        assert!(reads.attempts().all(|path| path.starts_with(&class)));
    }
    #[test]
    fn unsupported_nested_directories_do_not_inherit_device_schemas() {
        let (_temp, root, _, _) = fixture("devfreq", "memory-controller", &[]);
        let nested = root.join("userspace");
        std::fs::create_dir(&nested).unwrap();
        assert!(rules(&nested).is_empty());
        assert!(rules(Path::new("/unregistered/rfkill0")).is_empty());
    }
}
