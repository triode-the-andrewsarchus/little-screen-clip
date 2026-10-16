//! IIO registration/configuration metadata; no direct samples, triggers or streams.
use super::{
    Document,
    attributes::{self, Format, Rule},
    buses::{self, Topology},
    collector::Reads,
    model::{Fact, Outcome},
};
use std::{collections::BTreeSet, path::Path};
#[derive(Clone, Copy, PartialEq, Eq)]
enum Object {
    Device,
    Trigger,
    Buffer,
    Scan,
    TriggerSelection,
}
fn registration(path: &Path) -> Option<Object> {
    let subsystem = path.join("subsystem").canonicalize().ok()?;
    if subsystem.file_name()? != "iio" || subsystem.parent()?.file_name()? != "bus" {
        return None;
    }
    let name = path.file_name()?.to_str()?;
    if buses::numbered(name, "iio:device") {
        Some(Object::Device)
    } else if buses::numbered(name, "trigger") {
        Some(Object::Trigger)
    } else {
        None
    }
}
fn object(path: &Path) -> Option<Object> {
    if let Some(kind) = registration(path) {
        return Some(kind);
    }
    if registration(path.parent()?) != Some(Object::Device) {
        return None;
    }
    match path.file_name()?.to_str()? {
        "buffer" => Some(Object::Buffer),
        "scan_elements" => Some(Object::Scan),
        "trigger" => Some(Object::TriggerSelection),
        name if buses::numbered(name, "buffer") => Some(Object::Buffer),
        _ => None,
    }
}
const CHANNEL_TYPES: &[&str] = &[
    "voltage",
    "altvoltage",
    "current",
    "temp",
    "accel",
    "anglvel",
    "magn",
    "intensity",
    "illuminance",
    "pressure",
    "humidityrelative",
    "proximity",
    "distance",
    "timestamp",
    "count",
    "position",
    "velocity",
    "power",
    "energy",
    "capacitance",
    "resistance",
    "phase",
    "incli",
    "rot",
    "deltaangl",
    "deltavelocity",
    "activity",
    "steps",
];
fn channel_name(name: &str) -> bool {
    let Some(stem) = name
        .strip_prefix("in_")
        .or_else(|| name.strip_prefix("out_"))
    else {
        return false;
    };
    let end = stem
        .find(|c: char| c.is_ascii_digit() || c == '_' || c == '-')
        .unwrap_or(stem.len());
    CHANNEL_TYPES.contains(&&stem[..end])
}
pub(super) fn metadata_rule(path: &Path, name: &str) -> Option<Rule> {
    let kind = object(path)?;
    let (channel, field) = name.rsplit_once('_')?;
    if !channel_name(channel) {
        return None;
    }
    let (label, format) = match (kind, field) {
        (Object::Device, "label") => ("Channel label", Format::Text),
        (Object::Device, "scale") => ("Reported channel scale", Format::Text),
        (Object::Device, "offset") => ("Reported channel offset", Format::Text),
        (Object::Buffer | Object::Scan, "index") => ("Scan position index", Format::Unsigned),
        (Object::Buffer | Object::Scan, "type") => ("Scan storage format", Format::Text),
        (Object::Buffer | Object::Scan, "en") => (
            "Configured scan inclusion",
            Format::Boolean("Included", "Excluded"),
        ),
        _ => return None,
    };
    Some(Rule::new(name, &format!("{channel} · {label}"), format))
}
fn inventory(path: &Path) -> (Vec<String>, Outcome) {
    let Ok(entries) = std::fs::read_dir(path) else {
        return (Vec::new(), Outcome::Unsupported);
    };
    let (paths, coverage) = buses::select_children(
        entries,
        |entry| entry.file_type().is_ok_and(|kind| kind.is_file()),
        64,
    );
    let names = paths
        .into_iter()
        .filter_map(|path| path.file_name()?.to_str().map(str::to_string))
        .collect();
    (names, coverage)
}
pub(super) fn rules(path: &Path) -> Vec<Rule> {
    use Format::{Text as T, Unsigned as U};
    let specs: &[(&str, &str, Format)] = match object(path) {
        Some(Object::Device) => &[
            ("name", "IIO device name", T),
            ("label", "IIO device label", T),
            ("current_timestamp_clock", "Configured timestamp clock", T),
            (
                "sampling_frequency",
                "Configured sampling frequency (Hz)",
                T,
            ),
            (
                "sampling_frequency_available",
                "Available sampling frequencies (Hz)",
                T,
            ),
        ],
        Some(Object::Trigger) => &[
            ("name", "Registered IIO trigger name", T),
            ("sampling_frequency", "Configured trigger frequency (Hz)", T),
        ],
        Some(Object::Buffer) => &[
            (
                "enable",
                "Configured buffer enable",
                Format::Boolean("Enabled", "Disabled"),
            ),
            ("length", "Configured buffer length (scans)", U),
            ("watermark", "Configured buffer watermark (scans)", U),
        ],
        Some(Object::TriggerSelection) => &[("current_trigger", "Selected trigger name", T)],
        Some(Object::Scan) | None => &[],
    };
    let mut rules = attributes::materialize_specs(specs);
    if matches!(
        object(path),
        Some(Object::Device | Object::Buffer | Object::Scan)
    ) {
        rules.extend(
            inventory(path)
                .0
                .iter()
                .filter_map(|name| metadata_rule(path, name))
                .take(24),
        );
    }
    rules
}
pub(super) fn collect(
    document: &mut Document,
    root: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let Some(kind) = object(root) else {
        return;
    };
    for rule in rules(root) {
        reads.observe(document, root, &rule);
        checkpoint(document);
    }
    document.title = match kind {
        Object::Device => "IIO channel metadata",
        Object::Trigger => "IIO trigger metadata (not fired)",
        Object::Buffer => "IIO buffer configuration",
        Object::Scan => "IIO scan layout metadata",
        Object::TriggerSelection => "IIO trigger selection metadata",
    }
    .into();
    let (names, mut coverage) = inventory(root);
    let mut channels = names
        .iter()
        .filter_map(|name| name.rsplit_once('_').map(|(channel, _)| channel))
        .filter(|name| channel_name(name))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if channels.len() > 24 {
        channels.truncate(24);
        coverage = Outcome::Truncated;
    }
    if names
        .iter()
        .filter(|name| metadata_rule(root, name).is_some())
        .count()
        > 24
    {
        coverage = Outcome::Truncated;
    }
    let mut summary = root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    if let Some(value) = document
        .facts
        .iter()
        .find(|fact| fact.source == root.join("name"))
        .and_then(|fact| fact.value.as_deref())
    {
        summary.push_str(&format!(" · {value}"));
    }
    summary.push_str(&format!(
        " · {} represented channel stems · {coverage}",
        channels.len()
    ));
    let mut topology = Topology::new("IIO device/configuration relationships", summary);
    topology.coverage = coverage.clone();
    let parents = buses::ancestors(root, 12, |path| {
        registration(path).is_some()
            || matches!(
                buses::subsystem(path).as_deref(),
                Some("i2c" | "spi" | "platform" | "pci" | "usb")
            )
    });
    let parent = buses::ancestor_chain(&mut topology, document, parents, None);
    let title = document.title.clone();
    let anchor = buses::add(&mut topology, document, root.into(), title, parent);
    if kind == Object::Device {
        let (children, status) =
            buses::select_descendants(root, 1, 16, |path| path != root && object(path).is_some());
        if status != Outcome::Observed {
            topology.coverage = status;
        }
        for child in children {
            buses::add_descendant(&mut topology, document, child, anchor);
        }
    }
    let mut fact = Fact::text(
        "IIO channel/configuration inventory",
        channels.join(" · "),
        root.into(),
    );
    fact.outcome = coverage;
    document.facts.push(fact);
    buses::link_relations(
        document,
        root,
        &["device", "driver", "firmware_node", "of_node"],
        "IIO",
    );
    buses::access_node(document, root, "IIO access interface (unopened)");
    document.facts.push(Fact::text("IIO observation policy","Registration and exposed configuration metadata only; channel stems are not complete physical-channel identities. Scale/offset/frequency configuration is not a captured sample or activity proof. No raw/input/processed samples, data-ready values, events, device/buffer streams, trigger firing, capture enable, configuration writes or ioctls",root.into()));
    document.topology = Some(topology);
    checkpoint(document);
}

#[cfg(test)]
mod tests {
    use super::super::collector::registered_fixture;
    use super::*;
    use std::{collections::BTreeMap, os::unix::fs::symlink};
    fn device(values: &[(&str, &str)]) -> (tempfile::TempDir, std::path::PathBuf, Reads, Document) {
        let (temp, root, reads, doc) = registered_fixture("iio", "iio:device0", values);
        std::fs::remove_file(root.join("subsystem")).unwrap();
        let bus = temp.path().join("bus/iio");
        std::fs::create_dir_all(&bus).unwrap();
        symlink(bus, root.join("subsystem")).unwrap();
        (temp, root, reads, doc)
    }
    #[test]
    fn device_metadata_does_not_capture_raw_or_processed_samples() {
        let (_temp, root, mut reads, mut doc) = device(&[
            ("name", "ADC"),
            ("in_voltage0_label", "Input"),
            ("in_voltage0_scale", "0.25"),
            ("in_voltage0_offset", "-1"),
            ("in_voltage0_raw", "100"),
            ("in_temp_input", "25000"),
        ]);
        collect(&mut doc, &root, &mut reads, &mut |_| {});
        assert_eq!(doc.title, "IIO channel metadata");
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "in_voltage0 · Reported channel scale"
                    && fact.raw.as_deref() == Some("0.25"))
        );
        assert!(reads.attempts().all(|path| {
            !path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| name.ends_with("_raw") || name.ends_with("_input"))
        }));
        assert!(metadata_rule(&root, "in_voltage0_raw").is_none());
        assert!(metadata_rule(&root, "in_temp_input").is_none());
    }
    #[test]
    fn legacy_scan_and_numbered_buffers_use_owner_scopes_without_enabling_capture() {
        let (_temp, root, _, mut doc) = device(&[]);
        let buffer = root.join("buffer0");
        let scan = root.join("scan_elements");
        for dir in [&buffer, &scan] {
            std::fs::create_dir(dir).unwrap();
            std::fs::write(dir.join("in_voltage0_type"), "le:s12/16>>0").unwrap();
            std::fs::write(dir.join("in_voltage0_index"), "3").unwrap();
            std::fs::write(dir.join("in_voltage0_en"), "1").unwrap();
        }
        let mut reads = Reads::fixture(BTreeMap::from([
            (buffer.join("enable"), b"0".to_vec()),
            (buffer.join("in_voltage0_type"), b"le:s12/16>>0".to_vec()),
            (scan.join("in_voltage0_index"), b"3".to_vec()),
        ]));
        collect(&mut doc, &buffer, &mut reads, &mut |_| {});
        assert_eq!(doc.title, "IIO buffer configuration");
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "Configured buffer enable"
                    && fact.value.as_deref() == Some("Disabled"))
        );
        assert!(metadata_rule(&buffer, "in_voltage0_raw").is_none());
        let mut doc = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        collect(&mut doc, &scan, &mut reads, &mut |_| {});
        assert_eq!(doc.title, "IIO scan layout metadata");
        assert!(reads.attempts().all(|path| {
            !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == "data" || name == "data_available")
        }));
    }
    #[test]
    fn trigger_selection_and_unregistered_nested_paths_do_not_fire_triggers() {
        let (_temp, root, _, mut doc) = device(&[]);
        let trigger = root.join("trigger");
        std::fs::create_dir(&trigger).unwrap();
        let mut reads = Reads::fixture(BTreeMap::from([(
            trigger.join("current_trigger"),
            b"timer".to_vec(),
        )]));
        collect(&mut doc, &trigger, &mut reads, &mut |_| {});
        assert_eq!(doc.observed_raw("Selected trigger name"), Some("timer"));
        assert!(
            reads
                .attempts()
                .all(|path| path.file_name() == Some(std::ffi::OsStr::new("current_trigger")))
        );
        let vendor = root.join("vendor");
        std::fs::create_dir(&vendor).unwrap();
        assert!(rules(&vendor).is_empty());
        assert!(rules(Path::new("/unregistered/iio:device0")).is_empty());
    }
    #[test]
    fn large_channel_inventory_is_bounded_but_explicit_metadata_remains_inspectable() {
        let (_temp, root, mut reads, mut doc) = device(&[]);
        for index in 0..40 {
            std::fs::write(root.join(format!("in_voltage{index}_scale")), "1").unwrap();
        }
        collect(&mut doc, &root, &mut reads, &mut |_| {});
        assert_eq!(doc.topology.as_ref().unwrap().coverage, Outcome::Truncated);
        assert!(rules(&root).len() <= 29);
        assert!(metadata_rule(&root, "in_voltage99_scale").is_some());
        assert!(metadata_rule(&root, "in_vendor0_register").is_none());
    }
}
