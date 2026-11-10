//! PTP capabilities and stored PPS observations; no clock-device or FIFO reads.
use super::{
    Document,
    attributes::{self, Format, Rule},
    buses::{self, Topology},
    collector::Reads,
    model::Fact,
};
use std::path::Path;
fn kind(path: &Path) -> Option<bool> {
    let name = path.file_name()?.to_str()?;
    let subsystem = path.join("subsystem").canonicalize().ok()?;
    if subsystem.parent()?.file_name()? != "class" {
        return None;
    }
    let prefix = match subsystem.file_name()?.to_str()? {
        "ptp" => "ptp",
        "pps" => "pps",
        _ => return None,
    };
    let id = name.strip_prefix(prefix)?;
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) && id.parse::<u32>().is_ok())
        .then_some(prefix == "ptp")
}
pub(super) fn rules(path: &Path) -> Vec<Rule> {
    use Format::{Boolean as B, Text as T, Unsigned as U};
    let specs: &[(&str, &str, Format)] = match kind(path) {
        Some(true) => &[
            ("clock_name", "PTP clock friendly name", T),
            (
                "max_adjustment",
                "Maximum frequency adjustment",
                Format::UnsignedScale(1.0, "ppb"),
            ),
            ("n_alarms", "Alarm capability count", U),
            (
                "n_external_timestamps",
                "External timestamp channel count",
                U,
            ),
            ("n_periodic_outputs", "Periodic output capability count", U),
            ("n_programmable_pins", "Programmable pin count", U),
            ("n_pins", "Legacy pin-count attribute", U),
            (
                "pps_available",
                "PPS-to-host capability",
                B("Supported", "Not supported"),
            ),
            ("n_vclocks", "Configured virtual-clock count", U),
            ("max_vclocks", "Virtual-clock count limit", U),
        ],
        Some(false) => &[
            ("name", "PPS source name", T),
            ("path", "Reported PPS device path", T),
            ("mode", "Supported PPS mode bits (hex)", T),
            (
                "echo",
                "PPS echo capability",
                B("Supported", "Not supported"),
            ),
            (
                "assert",
                "Stored assert timestamp/sequence",
                Format::PpsTimestamp,
            ),
            (
                "clear",
                "Stored clear timestamp/sequence",
                Format::PpsTimestamp,
            ),
        ],
        None => &[],
    };
    attributes::materialize_specs(specs)
}
pub(super) fn collect(
    document: &mut Document,
    root: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let Some(ptp) = kind(root) else {
        return;
    };
    for rule in rules(root) {
        reads.observe(document, root, &rule);
        checkpoint(document);
    }
    document.title = if ptp {
        "PTP clock capabilities"
    } else {
        "Stored PPS observations"
    }
    .into();
    let mut summary = root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let fields: &[(&str, &str)] = if ptp {
        &[
            ("clock_name", "Clock"),
            ("max_adjustment", "Adjustment limit"),
            ("n_external_timestamps", "External channels"),
            ("n_periodic_outputs", "Outputs"),
            ("pps_available", "PPS capability"),
        ]
    } else {
        &[("name", "Source"), ("assert", "Assert"), ("clear", "Clear")]
    };
    for (field, label) in fields {
        if let Some(fact) = document.observed_attribute(root, field) {
            if let Some(value) = &fact.value {
                summary.push_str(&format!(" · {label}: {value}"));
            }
        }
    }
    let mut topology = Topology::new("Precision clock/source relationships", summary);
    let ancestors = buses::ancestors(root, 12, |path| {
        kind(path).is_some()
            || matches!(
                buses::subsystem(path).as_deref(),
                Some("pci" | "usb" | "platform" | "net")
            )
    });
    let parent = buses::ancestor_chain(&mut topology, document, ancestors, None);
    buses::add(
        &mut topology,
        document,
        root.into(),
        document.title.clone(),
        parent,
    );
    buses::link_relations(
        document,
        root,
        &["device", "driver", "firmware_node", "of_node"],
        "Clock/source",
    );
    buses::access_node(document, root, "Clock/source access interface (unopened)");
    let policy = if ptp {
        "Capability counts/friendly names do not prove synchronization, active outputs or unique physical identity. Clock devices, FIFO and pin assignments stay unread; no timestamp/PPS/output enable, virtual-clock creation, adjustment or ioctl calls"
    } else {
        "Stored event snapshots only; sequential fields are not an atomic capture. Empty/zero snapshots do not establish synchronization or source health; mode describes exposed source capabilities. No device fetch, poll, echo execution, parameter changes or kernel-consumer binding"
    };
    document.facts.push(Fact::text(
        "Precision clock observation policy",
        policy,
        root.into(),
    ));
    document.topology = Some(topology);
    checkpoint(document);
}
pub(super) fn timestamp(raw: &str) -> std::io::Result<String> {
    let invalid = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Malformed stored PPS timestamp/sequence",
        )
    };
    if raw.is_empty() {
        return Ok("No exposed stored event timestamp".into());
    }
    let (time, sequence) = raw.split_once('#').ok_or_else(invalid)?;
    let (seconds, nanos) = time.split_once('.').ok_or_else(invalid)?;
    let seconds = seconds.parse::<i64>().map_err(|_| invalid())?;
    if nanos.len() != 9 || !nanos.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let nanos = nanos.parse::<u32>().map_err(|_| invalid())?;
    if nanos >= 1_000_000_000 {
        return Err(invalid());
    }
    let sequence = sequence
        .parse::<u32>()
        .or_else(|_| sequence.parse::<i32>().map(|value| value as u32))
        .map_err(|_| invalid())?;
    let time = if seconds < 0 {
        format!("seconds {seconds}, nanoseconds {nanos:09}")
    } else {
        format!("{seconds}.{nanos:09} s")
    };
    Ok(format!("{time} · sequence {sequence} (stored snapshot)"))
}

#[cfg(test)]
mod tests {
    use super::super::{collector::registered_fixture, model::Outcome};
    use super::*;
    #[test]
    fn ptp_capabilities_do_not_open_event_queues_or_enable_outputs() {
        let (_temp, root, mut reads, mut doc) = registered_fixture(
            "ptp",
            "ptp0",
            &[
                ("clock_name", "Clock"),
                ("max_adjustment", "500000"),
                ("n_external_timestamps", "2"),
                ("pps_available", "1"),
            ],
        );
        collect(&mut doc, &root, &mut reads, &mut |_| {});
        assert_eq!(doc.title, "PTP clock capabilities");
        assert!(
            doc.topology
                .as_ref()
                .unwrap()
                .summary
                .contains("500000 ppb")
        );
        assert_eq!(doc.observed_raw("PPS-to-host capability"), Some("1"));
        doc.facts
            .iter_mut()
            .find(|fact| fact.source == root.join("clock_name"))
            .unwrap()
            .retained = true;
        doc.facts
            .iter_mut()
            .find(|fact| fact.source == root.join("max_adjustment"))
            .unwrap()
            .outcome = super::super::model::Outcome::PermissionDenied;
        collect(&mut doc, &root, &mut reads, &mut |_| {});
        let summary = &doc.topology.as_ref().unwrap().summary;
        assert!(!summary.contains("Clock: Clock"));
        assert!(!summary.contains("500000 ppb"));
        assert!(reads.attempts().all(|path| !matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("fifo" | "pps_enable" | "extts_enable" | "period" | "max_phase_adjustment")
        )));
    }
    #[test]
    fn pps_stored_timestamps_preserve_nanos_signed_sequences_and_empty_snapshots() {
        assert_eq!(
            timestamp("1.000000001#-1").unwrap(),
            "1.000000001 s · sequence 4294967295 (stored snapshot)"
        );
        assert!(timestamp("").unwrap().contains("No exposed"));
        assert!(
            timestamp("-1.500000000#1")
                .unwrap()
                .contains("seconds -1, nanoseconds 500000000")
        );
        for raw in [
            "1.1#0",
            "1.1000000000#0",
            "1.-00000001#0",
            "1.000000000#4294967296",
            "garbage",
        ] {
            assert!(timestamp(raw).is_err());
        }
        let (_temp, root, mut reads, mut doc) = registered_fixture(
            "pps",
            "pps0",
            &[
                ("name", "Source"),
                ("assert", "0.000000000#0"),
                ("clear", ""),
                ("mode", "1151"),
                ("echo", "0"),
            ],
        );
        collect(&mut doc, &root, &mut reads, &mut |_| {});
        assert_eq!(doc.title, "Stored PPS observations");
        assert_eq!(
            doc.observed_raw("Supported PPS mode bits (hex)"),
            Some("1151")
        );
        assert_eq!(
            doc.observed_raw("Stored assert timestamp/sequence"),
            Some("0.000000000#0")
        );
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "Stored clear timestamp/sequence"
                    && fact.value.as_deref() == Some("No exposed stored event timestamp"))
        );
    }
    #[test]
    fn malformed_pps_data_and_unknown_pin_directories_stay_explicit() {
        let (_temp, root, mut reads, mut doc) =
            registered_fixture("pps", "pps0", &[("assert", "bad")]);
        collect(&mut doc, &root, &mut reads, &mut |_| {});
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "Stored assert timestamp/sequence"
                    && fact.outcome == Outcome::Malformed)
        );
        let pins = root.join("pins");
        std::fs::create_dir(&pins).unwrap();
        assert!(rules(&pins).is_empty());
        assert!(rules(Path::new("/unregistered/ptp0")).is_empty());
    }
}
