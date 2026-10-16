//! Remote-processor state and registered messaging metadata; no device I/O.
use super::{
    Document,
    attributes::{self, Format, Rule},
    buses::{self, Topology},
    collector::Reads,
    model::Fact,
};
use std::path::Path;
#[derive(Clone, Copy, PartialEq, Eq)]
enum Object {
    Processor,
    Channel,
    Endpoint,
    Control,
}

fn object(path: &Path) -> Option<Object> {
    let subsystem = path.join("subsystem").canonicalize().ok()?;
    let name = path.file_name()?.to_str()?;
    match (
        subsystem.file_name()?.to_str()?,
        subsystem.parent()?.file_name()?.to_str()?,
    ) {
        ("remoteproc", "class") if buses::numbered(name, "remoteproc") => Some(Object::Processor),
        ("rpmsg", "bus") => Some(Object::Channel),
        ("rpmsg", "class") if buses::numbered(name, "rpmsg_ctrl") => Some(Object::Control),
        ("rpmsg", "class") if buses::numbered(name, "rpmsg") => Some(Object::Endpoint),
        _ => None,
    }
}
pub(super) fn rules(path: &Path) -> Vec<Rule> {
    let specs: &[(&str, &str, Format)] = match object(path) {
        Some(Object::Processor) => &[
            ("name", "Remote processor identity", Format::Text),
            ("state", "Remote processor state", Format::Text),
            ("firmware", "Firmware name (kernel report)", Format::Text),
            ("recovery", "Recovery policy", Format::Text),
            ("coredump", "Coredump configuration", Format::Text),
        ],
        Some(Object::Channel) => &[
            ("name", "Registered channel name", Format::Text),
            ("src", "Registered local address", Format::RpmsgAddress),
            ("dst", "Registered remote address", Format::RpmsgAddress),
        ],
        Some(Object::Endpoint) => &[
            ("name", "Endpoint definition name", Format::Text),
            ("src", "Configured local address", Format::RpmsgAddress),
            ("dst", "Configured remote address", Format::RpmsgAddress),
        ],
        Some(Object::Control) | None => &[],
    };
    attributes::materialize_specs(specs)
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
    let title = match kind {
        Object::Processor => "Remote processor metadata",
        Object::Channel => "Registered RPMsg channel",
        Object::Endpoint => "RPMsg endpoint definition",
        Object::Control => "RPMsg endpoint control (unopened)",
    };
    document.title = title.into();
    let summary = buses::attribute_summary(document, root);
    let mut topology = Topology::new("Remote processor/messaging relationships", summary);
    let ancestors = buses::ancestors(root, 16, |path| {
        object(path).is_some()
            || matches!(
                buses::subsystem(path).as_deref(),
                Some("platform" | "pci" | "usb" | "virtio")
            )
    });
    let parent = buses::ancestor_chain(&mut topology, document, ancestors, None);
    let anchor = buses::add(&mut topology, document, root.into(), title.into(), parent);
    let (children, coverage) =
        buses::select_descendants(root, 4, 16, |path| path != root && object(path).is_some());
    topology.coverage = coverage;
    for child in children {
        buses::add_descendant(&mut topology, document, child, anchor);
    }
    buses::link_relations(
        document,
        root,
        &["device", "driver", "firmware_node", "of_node"],
        "Remote/messaging",
    );
    buses::access_node(
        document,
        root,
        "Remote/messaging access interface (unopened)",
    );
    document.facts.push(Fact::text("Remote/messaging observation policy","Registered/configured metadata only; channel/address exposure does not prove active traffic, responsiveness or a userspace endpoint's currently assigned address. Firmware is a kernel-reported name, not a loaded-image verification. No processor start/stop/recovery, firmware loads, dumps, endpoint creation, device opens, ioctls or message traffic",root.into()));
    document.topology = Some(topology);
    checkpoint(document);
}

#[cfg(test)]
mod tests {
    use super::super::{collector::registered_fixture, model::Value};
    use super::*;
    use std::{collections::BTreeMap, os::unix::fs::symlink};
    #[test]
    fn processor_metadata_never_loads_firmware_or_opens_coredump_payloads() {
        let (_temp, root, mut reads, mut doc) = registered_fixture(
            "remoteproc",
            "remoteproc0",
            &[
                ("name", "DSP"),
                ("state", "offline"),
                ("firmware", "firmware.elf"),
                ("recovery", "disabled"),
                ("coredump", "inline"),
            ],
        );
        collect(&mut doc, &root, &mut reads, &mut |_| {});
        assert_eq!(doc.title, "Remote processor metadata");
        assert!(doc.topology.as_ref().unwrap().summary.contains("offline"));
        assert_eq!(
            doc.observed_raw("Firmware name (kernel report)"),
            Some("firmware.elf")
        );
        assert_eq!(doc.observed_raw("Coredump configuration"), Some("inline"));
        assert!(
            reads
                .attempts()
                .all(|path| path.parent() == Some(root.as_path()))
        );
        assert!(
            reads
                .attempts()
                .all(|path| path.file_name() != Some(std::ffi::OsStr::new("data")))
        );
    }
    #[test]
    fn channel_and_class_endpoint_addresses_preserve_signed_rendering_and_wildcards() {
        let (temp, root, _, mut doc) = registered_fixture("rpmsg", "virtio0.service.-1.42", &[]);
        std::fs::remove_file(root.join("subsystem")).unwrap();
        let bus = temp.path().join("bus/rpmsg");
        std::fs::create_dir_all(&bus).unwrap();
        symlink(&bus, root.join("subsystem")).unwrap();
        let mut reads = Reads::fixture(BTreeMap::from([
            (root.join("name"), b"service".to_vec()),
            (root.join("src"), b"-1".to_vec()),
            (root.join("dst"), b"42".to_vec()),
        ]));
        collect(&mut doc, &root, &mut reads, &mut |_| {});
        assert_eq!(doc.title, "Registered RPMsg channel");
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "Registered local address"
                    && fact.semantic == Some(Value::Integer(u32::MAX.into())))
        );
        let (_temp, root, mut reads, mut doc) = registered_fixture(
            "rpmsg",
            "rpmsg0",
            &[("src", "-2147483648"), ("dst", "4294967295")],
        );
        collect(&mut doc, &root, &mut reads, &mut |_| {});
        assert_eq!(doc.title, "RPMsg endpoint definition");
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "Configured local address"
                    && fact.value.as_deref() == Some("0x80000000 (2147483648)"))
        );
        let rule = rules(&root)
            .into_iter()
            .find(|rule| rule.name == "src")
            .unwrap();
        for raw in ["4294967296", "-2147483649", "garbage"] {
            assert!(attributes::parse(&rule, raw).is_err());
        }
    }
    #[test]
    fn controller_has_no_endpoint_attributes_and_unknown_nested_objects_stay_unread() {
        let (_temp, root, mut reads, mut doc) = registered_fixture("rpmsg", "rpmsg_ctrl0", &[]);
        collect(&mut doc, &root, &mut reads, &mut |_| {});
        assert_eq!(doc.title, "RPMsg endpoint control (unopened)");
        assert_eq!(reads.attempts().count(), 0);
        let nested = root.join("private");
        std::fs::create_dir(&nested).unwrap();
        assert!(rules(&nested).is_empty());
        assert!(rules(std::path::Path::new("/unregistered/remoteproc0")).is_empty());
    }
    #[test]
    fn hierarchy_is_metadata_only_and_bounded() {
        let (temp, root, mut reads, mut doc) = registered_fixture("remoteproc", "remoteproc0", &[]);
        let bus = temp.path().join("bus/rpmsg");
        std::fs::create_dir_all(&bus).unwrap();
        for index in 0..20 {
            let child = root.join(format!("channel{index}"));
            std::fs::create_dir(&child).unwrap();
            symlink(&bus, child.join("subsystem")).unwrap();
        }
        let endpoint = root.join("channel0/intermediate/rpmsg0");
        std::fs::create_dir_all(&endpoint).unwrap();
        let class = temp.path().join("class/rpmsg");
        std::fs::create_dir_all(&class).unwrap();
        symlink(&class, endpoint.join("subsystem")).unwrap();
        collect(&mut doc, &root, &mut reads, &mut |_| {});
        let topology = doc.topology.as_ref().unwrap();
        let nested = topology
            .nodes
            .iter()
            .find(|node| node.path == endpoint)
            .unwrap();
        assert_eq!(
            topology.nodes[nested.parent.unwrap()].path,
            root.join("channel0")
        );
        assert_eq!(topology.nodes.len(), 17);
        assert_eq!(topology.coverage, super::super::model::Outcome::Truncated);
        assert!(
            reads
                .attempts()
                .all(|path| path.parent() == Some(root.as_path()))
        );
    }
}
