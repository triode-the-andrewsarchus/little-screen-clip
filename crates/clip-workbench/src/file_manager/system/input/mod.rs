//! Input capability metadata and HID descriptor summaries, never event streams.
mod bitmap;
mod hid;
use super::{
    Document,
    attributes::{self, Context},
    buses::{self, Topology},
    collector::Reads,
    model::{Fact, Outcome},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
};

fn numbered(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit()))
}
fn role(path: &Path) -> Option<&'static str> {
    let subsystem = buses::subsystem(path)?;
    let name = path.file_name()?.to_str()?;
    match subsystem.as_str() {
        "hid" => Some("HID device"),
        "hidraw" => Some("Raw HID interface"),
        "input" if name == "mice" => Some("Aggregate mouse interface"),
        "input" => [
            ("input", "Input device"),
            ("event", "Event interface"),
            ("mouse", "Mouse interface"),
            ("js", "Joystick interface"),
        ]
        .into_iter()
        .find(|(prefix, _)| numbered(name, prefix))
        .map(|(_, role)| role),
        _ => None,
    }
}
fn input_root(sys: &Path) -> Option<PathBuf> {
    sys.ancestors()
        .take(6)
        .find(|path| role(path) == Some("Input device"))
        .map(Path::to_path_buf)
}
fn hid_root(sys: &Path) -> Option<PathBuf> {
    sys.ancestors()
        .take(8)
        .find(|path| buses::subsystem(path).as_deref() == Some("hid"))
        .map(Path::to_path_buf)
}

fn physical_group(input: Option<&Path>, hid: Option<&Path>) -> (Option<PathBuf>, Option<PathBuf>) {
    let anchor = hid.or(input);
    let usb = anchor.and_then(buses::usb_parent);
    let group = usb.clone().or_else(|| anchor.map(Path::to_path_buf));
    (usb, group)
}

/// Uses the same USB/HID/input grouping as the individual interface preview.
pub(super) fn directory_group(sys: &Path) -> Option<(Option<PathBuf>, String)> {
    let role = role(sys)?;
    let (_, group) = physical_group(input_root(sys).as_deref(), hid_root(sys).as_deref());
    Some((group, role.into()))
}
fn decoded(
    document: &mut Document,
    fact: &Fact,
    label: &str,
    decode: fn(&str) -> io::Result<String>,
) -> Option<String> {
    let mut result = match fact.raw.as_deref() {
        Some(raw) => match decode(raw) {
            Ok(value) => Fact::text(label, value, fact.source.clone()),
            Err(error) => Fact::failure(label, fact.source.clone(), error),
        },
        None => {
            let mut missing = fact.clone();
            missing.label = label.into();
            missing
        }
    };
    result.origin = "decoded input capability metadata".into();
    result.raw = fact.raw.clone();
    result.observed_ms = fact.observed_ms;
    let value = result.value.clone();
    document.facts.push(result);
    value
}
fn input_metadata(
    document: &mut Document,
    root: &Path,
    reads: &mut Reads,
) -> (Option<String>, Option<String>, Option<String>) {
    let mut name = None;
    let mut bus = None;
    let mut events = None;
    for rule in attributes::rules(&Context::discover(root)) {
        let fact = reads.observe(document, root, &rule);
        match rule.name.as_str() {
            "name" => name = fact.value,
            "id/bustype" => bus = decoded(document, &fact, "Input bus", bitmap::bus),
            "capabilities/ev" => {
                events = decoded(document, &fact, "Input event categories", bitmap::events)
            }
            _ => {}
        }
    }
    (name, bus, events)
}
fn hid_fields(raw: &str) -> io::Result<BTreeMap<&str, &str>> {
    let mut fields = BTreeMap::new();
    for line in raw.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if !matches!(key, "HID_NAME" | "HID_ID" | "HID_PHYS" | "HID_UNIQ") {
            continue;
        }
        if fields.insert(key, value).is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Repeated HID identity property",
            ));
        }
    }
    Ok(fields)
}
fn hid_id(raw: &str) -> io::Result<(String, String, String)> {
    let parts = raw.split(':').collect::<Vec<_>>();
    if parts.len() != 3
        || parts[0].len() != 4
        || parts[1].len() != 8
        || parts[2].len() != 8
        || parts
            .iter()
            .any(|part| !part.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid HID bus/vendor/product identity",
        ));
    }
    let parse = |part| {
        u32::from_str_radix(part, 16)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid HID identity number"))
    };
    Ok((
        bitmap::bus(parts[0])?,
        format!("0x{:04x}", parse(parts[1])?),
        format!("0x{:04x}", parse(parts[2])?),
    ))
}
fn hid_metadata(
    document: &mut Document,
    root: &Path,
    reads: &mut Reads,
) -> (Option<String>, Option<String>) {
    let source = root.join("uevent");
    let mut name = None;
    let mut bus = None;
    match reads.read(&source).and_then(|raw| {
        hid_fields(&raw).map(|fields| {
            fields
                .iter()
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect::<BTreeMap<_, _>>()
        })
    }) {
        Ok(fields) => {
            for (key, label) in [
                ("HID_NAME", "HID name"),
                ("HID_PHYS", "HID physical path"),
                ("HID_UNIQ", "HID unique ID"),
            ] {
                if let Some(value) = fields.get(key) {
                    let mut fact = Fact::text(label, value, source.clone());
                    fact.origin = format!("kernel HID uevent {key}");
                    document.facts.push(fact);
                    if key == "HID_NAME" && !value.is_empty() {
                        name = Some(value.clone());
                    }
                }
            }
            if let Some(raw) = fields.get("HID_ID") {
                match hid_id(raw) {
                    Ok((bus_name, vendor, product)) => {
                        bus = Some(bus_name.clone());
                        for (label, value) in [
                            ("HID bus", bus_name),
                            ("HID vendor ID", vendor),
                            ("HID product ID", product),
                        ] {
                            let mut fact = Fact::text(label, value, source.clone());
                            fact.origin = "kernel HID uevent HID_ID".into();
                            fact.raw = Some(raw.clone());
                            document.facts.push(fact);
                        }
                    }
                    Err(error) => document
                        .facts
                        .push(Fact::failure("HID identity", source, error)),
                }
            }
        }
        Err(error) => document
            .facts
            .push(Fact::failure("HID identity", source, error)),
    }
    (name, bus)
}
fn descriptor(document: &mut Document, root: &Path, reads: &mut Reads) {
    let source = root.join("report_descriptor");
    let mut fact = match reads
        .hid_descriptor(&source)
        .and_then(|bytes| hid::summarize(&bytes))
    {
        Ok(value) => Fact::text("HID declared reports", value, source),
        Err(error) => Fact::failure("HID declared reports", source, error),
    };
    fact.origin = "decoded sysfs HID report descriptor; declared lengths, not live reports".into();
    document.facts.push(fact);
}
pub(super) fn collect(
    document: &mut Document,
    sys: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let Some(selected_role) = role(sys) else {
        return;
    };
    document.title = if document
        .path
        .file_name()
        .is_some_and(|name| name == "report_descriptor")
    {
        "HID report descriptor"
    } else {
        selected_role
    }
    .into();
    let input = input_root(sys);
    let hid = hid_root(sys);
    let (mut name, mut bus, events) = input
        .as_deref()
        .map(|root| input_metadata(document, root, reads))
        .unwrap_or_default();
    if let Some(root) = &hid {
        let (hid_name, hid_bus) = hid_metadata(document, root, reads);
        if name.is_none() {
            name = hid_name;
        }
        if bus.is_none() {
            bus = hid_bus;
        }
    }
    let owner = hid.clone().or_else(|| {
        input
            .as_deref()
            .and_then(|root| root.join("device").canonicalize().ok())
    });
    if let Some(owner) = &owner {
        if let Some(driver) = buses::driver(owner) {
            document
                .facts
                .push(Fact::text("Input driver", driver, owner.join("driver")));
        }
    }
    let name = name.filter(|name| !name.is_empty()).unwrap_or_else(|| {
        sys.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    });
    let mut summary = format!("{name} · {selected_role}");
    if let Some(bus) = bus {
        summary.push_str(&format!(" · {bus}"));
    }
    if let Some(events) = events {
        summary.push_str(&format!(" · {events}"));
    }
    let (usb, group) = physical_group(input.as_deref(), hid.as_deref());
    let topology = initial_topology(
        document,
        summary,
        [usb, owner, hid.clone(), input.clone(), Some(sys.into())]
            .into_iter()
            .flatten(),
    );
    document.topology = Some(topology);
    checkpoint(document);
    if let Some(group) = group {
        related(document, &group, sys);
    }
    if let Some(root) = hid {
        descriptor(document, &root, reads);
    }
    document.facts.push(Fact::text("Input inspection policy", "Advertised capabilities and registered interface associations only. No event stream, live key/button state, HID input report or device ioctl is read. Descriptor report lengths are declarations, not captured data.", sys.into()));
    checkpoint(document);
}
fn initial_topology(
    document: &mut Document,
    summary: String,
    paths: impl IntoIterator<Item = PathBuf>,
) -> Topology {
    let mut topology = Topology::new("Input access interfaces", summary);
    let mut parent = None;
    let mut seen = BTreeSet::new();
    for path in paths {
        if !seen.insert(path.clone()) {
            continue;
        }
        let label = format!(
            "{} · {}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            role(&path).unwrap_or("Registered hardware")
        );
        parent = Some(buses::add(
            &mut topology,
            document,
            path.clone(),
            label,
            parent,
        ));
        buses::access_node(document, &path, "Input access node");
    }
    topology
}
fn related(document: &mut Document, group: &Path, selected: &Path) {
    let Some(mut topology) = document.topology.take() else {
        return;
    };
    related_into(document, group, selected, &mut topology, 0);
    document.topology = Some(topology);
}

pub(super) fn related_into(
    document: &mut Document,
    group: &Path,
    selected: &Path,
    topology: &mut Topology,
    root: usize,
) {
    let (peers, coverage) = buses::select_descendants(group, 6, 32, |path| {
        path != selected && role(path).is_some()
    });
    if coverage != Outcome::Observed {
        topology.coverage = coverage;
    }
    for path in peers {
        if topology.nodes.iter().any(|node| node.path == path) {
            continue;
        }
        let Some(peer_role) = role(&path) else {
            topology.coverage = Outcome::Disappeared;
            continue;
        };
        let anchor = hid_root(&path).or_else(|| input_root(&path));
        if anchor
            .as_deref()
            .map(|root| buses::usb_parent(root).unwrap_or_else(|| root.into()))
            .as_deref()
            != Some(group)
        {
            continue;
        }
        let parent = path
            .ancestors()
            .skip(1)
            .find_map(|ancestor| topology.nodes.iter().position(|node| node.path == ancestor))
            .or(Some(root));
        buses::add(
            topology,
            document,
            path.clone(),
            format!(
                "{} · {peer_role}",
                path.file_name().unwrap_or_default().to_string_lossy()
            ),
            parent,
        );
        buses::access_node(document, &path, "Related input access node");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hid_identity_preserves_full_ids_and_rejects_ambiguous_metadata() {
        assert_eq!(
            hid_id("0003:00001E7D:00002E4A").unwrap(),
            ("USB (0x0003)".into(), "0x1e7d".into(), "0x2e4a".into())
        );
        assert!(hid_id("0003:1e7d:2e4a").is_err());
        assert!(hid_fields("HID_NAME=one\nHID_NAME=two").is_err());
        assert_eq!(
            hid_fields("HID_NAME=keyboard\nDRIVER=hid-generic")
                .unwrap()
                .len(),
            1
        );
    }
    #[test]
    fn non_usb_input_owner_is_navigable_without_merging_sibling_devices() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let class = tmp.path().join("input");
        std::fs::create_dir(&class).unwrap();
        let owner = tmp.path().join("platform-controller");
        let first = owner.join("input/input1");
        let second = owner.join("input/input2");
        let selected = first.join("event1");
        let sibling = second.join("event2");
        for path in [&first, &second, &selected, &sibling] {
            std::fs::create_dir_all(path).unwrap();
            symlink(&class, path.join("subsystem")).unwrap();
        }
        symlink(&owner, first.join("device")).unwrap();
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        let mut reads = Reads::fixture(BTreeMap::from([
            (first.join("name"), b"Platform buttons".to_vec()),
            (first.join("id/bustype"), b"0019".to_vec()),
        ]));
        collect(&mut document, &selected, &mut reads, &mut |_| {});
        let nodes = &document.topology.as_ref().unwrap().nodes;
        assert_eq!(nodes[0].path, owner);
        assert!(nodes[0].label.contains("Registered hardware"));
        assert!(!nodes[0].label.contains("USB"));
        assert_eq!(nodes[1].path, first);
        assert_eq!(nodes[1].parent, Some(0));
        assert!(
            document
                .relations
                .iter()
                .any(|relation| relation.target == owner)
        );
        assert!(
            !nodes
                .iter()
                .any(|node| node.path == sibling || node.path == second)
        );
        std::fs::remove_file(first.join("device")).unwrap();
        let mut missing = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        collect(
            &mut missing,
            &selected,
            &mut Reads::fixture(BTreeMap::new()),
            &mut |_| {},
        );
        assert_eq!(missing.topology.as_ref().unwrap().nodes[0].path, first);
        assert!(
            !missing
                .topology
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .any(|node| node.path == owner)
        );
    }
    #[test]
    fn event_and_raw_hid_previews_share_registered_interfaces_without_live_reads() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let physical = temp.path().join("1-2");
        let hid = physical.join("0003:1234:5678.0001");
        let input = hid.join("input/input9");
        let event = input.join("event9");
        let mouse = input.join("mouse2");
        let raw = hid.join("hidraw/hidraw2");
        for name in ["usb", "hid", "input", "hidraw"] {
            std::fs::create_dir(temp.path().join(name)).unwrap();
        }
        for (path, bus) in [
            (&physical, "usb"),
            (&hid, "hid"),
            (&input, "input"),
            (&event, "input"),
            (&mouse, "input"),
            (&raw, "hidraw"),
        ] {
            std::fs::create_dir_all(path).unwrap();
            symlink(temp.path().join(bus), path.join("subsystem")).unwrap();
        }
        std::fs::write(physical.join("idVendor"), "metadata marker only").unwrap();
        let values = BTreeMap::from([
            (input.join("name"), b"Test pointer".to_vec()),
            (input.join("id/bustype"), b"0003".to_vec()),
            (input.join("capabilities/ev"), b"7".to_vec()),
            (
                hid.join("uevent"),
                b"HID_NAME=Test pointer\nHID_ID=0003:00001234:00005678\nHID_PHYS=usb-test/input0"
                    .to_vec(),
            ),
            (
                hid.join("report_descriptor"),
                vec![0x75, 8, 0x95, 3, 0x81, 2],
            ),
        ]);
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        document.facts.clear();
        document.path = event.clone();
        let mut reads = Reads::fixture(values.clone());
        collect(&mut document, &event, &mut reads, &mut |_| {});
        assert_eq!(document.title, "Event interface");
        assert!(
            document
                .topology
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .any(|node| node.path == raw)
        );
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "Input event categories"
                    && fact.value.as_deref().unwrap_or("").contains("EV_REL"))
        );
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "HID declared reports"
                    && fact.value.as_deref().unwrap_or("").contains("24 bits"))
        );
        assert!(
            reads
                .attempts()
                .all(|path| path.starts_with(&input) || path == hid.join("uevent"))
        );
        assert!(
            reads
                .hid_descriptor(Path::new("/dev/input/event9"))
                .is_err()
        );
        let mut raw_document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        raw_document.path = raw.clone();
        collect(
            &mut raw_document,
            &raw,
            &mut Reads::fixture(values),
            &mut |_| {},
        );
        assert_eq!(raw_document.title, "Raw HID interface");
        assert!(
            raw_document
                .topology
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .any(|node| node.path == event)
        );
        assert_eq!(directory_group(&event).unwrap().0.as_ref(), Some(&physical));
        assert_eq!(directory_group(&raw).unwrap().0.as_ref(), Some(&physical));
        let mut directory_topology = Topology::new("Input directory", String::new());
        let parent = buses::add(
            &mut directory_topology,
            &mut document,
            physical.clone(),
            "Physical input group".into(),
            None,
        );
        related_into(
            &mut document,
            &physical,
            &physical,
            &mut directory_topology,
            parent,
        );
        assert!(directory_topology.nodes.iter().any(|node| node.path == raw));
        assert!(
            directory_topology
                .nodes
                .iter()
                .any(|node| node.path == event)
        );
    }
    #[test]
    fn aggregate_mouse_interface_does_not_gain_a_guessed_physical_identity() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let bus = temp.path().join("input");
        let mice = temp.path().join("virtual/mice");
        std::fs::create_dir(&bus).unwrap();
        std::fs::create_dir_all(&mice).unwrap();
        symlink(&bus, mice.join("subsystem")).unwrap();
        assert!(input_root(&mice).is_none());
        assert!(hid_root(&mice).is_none());
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        let mut reads = Reads::fixture(BTreeMap::new());
        collect(&mut document, &mice, &mut reads, &mut |_| {});
        assert_eq!(document.title, "Aggregate mouse interface");
        assert_eq!(document.topology.as_ref().unwrap().nodes.len(), 1);
        assert_eq!(reads.attempts().count(), 0);
    }
    #[test]
    fn derived_capability_failure_preserves_source_outcome_and_observation_age() {
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        let mut fact = Fact::failure(
            "Event capabilities",
            PathBuf::from("/sys/input/capabilities/ev"),
            io::Error::from(io::ErrorKind::PermissionDenied),
        );
        fact.observed_ms = 123;
        assert!(
            decoded(
                &mut document,
                &fact,
                "Input event categories",
                bitmap::events
            )
            .is_none()
        );
        let derived = document.facts.last().unwrap();
        assert_eq!(derived.outcome, Outcome::PermissionDenied);
        assert_eq!(derived.observed_ms, 123);
        assert_eq!(derived.source, fact.source);
    }
}
