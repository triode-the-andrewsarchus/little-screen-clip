//! Registered Type-C and Thunderbolt topology; no role, tunnel or firmware changes.
use super::{
    Document,
    attributes::{Format, Rule},
    buses::{self, Topology},
    collector::Reads,
    model::{Fact, Outcome},
};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Object {
    Port,
    Partner,
    Cable,
    Plug,
    AltMode,
    Domain,
    Router,
    Usb4Port,
}
fn digits(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}
fn typec_name(name: &str) -> Option<Object> {
    let rest = name.strip_prefix("port")?;
    let split = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    if !digits(&rest[..split]) {
        return None;
    }
    match &rest[split..] {
        "" => Some(Object::Port),
        "-partner" => Some(Object::Partner),
        "-cable" => Some(Object::Cable),
        "-cable-plug0" | "-cable-plug1" | "-plug0" | "-plug1" => Some(Object::Plug),
        suffix => {
            let (owner, index) = suffix.rsplit_once('.')?;
            if digits(index)
                && matches!(
                    owner,
                    "" | "-partner" | "-cable-plug0" | "-cable-plug1" | "-plug0" | "-plug1"
                )
            {
                Some(Object::AltMode)
            } else {
                None
            }
        }
    }
}
fn object(path: &Path) -> Option<Object> {
    let name = path.file_name()?.to_str()?;
    match buses::subsystem(path).as_deref()? {
        "typec" => typec_name(name),
        "thunderbolt" => {
            if name.strip_prefix("domain").is_some_and(digits) {
                return Some(Object::Domain);
            }
            if name.strip_prefix("usb4_port").is_some_and(digits) {
                return Some(Object::Usb4Port);
            }
            let (domain, route) = name.split_once('-')?;
            (digits(domain) && !route.is_empty() && route.bytes().all(|b| b.is_ascii_hexdigit()))
                .then_some(Object::Router)
        }
        _ => None,
    }
}

pub(super) fn rules(path: &Path) -> Vec<Rule> {
    use Format::{Selection as S, Text as T, Unsigned as U};
    let schema: &[(&str, &str, Format)] = match object(path) {
        Some(Object::Port) => &[
            ("power_role", "Power role", S),
            ("data_role", "Data role", S),
            ("port_type", "Port type", T),
            ("orientation", "Orientation", T),
            ("power_operation_mode", "Power operation mode", T),
            ("vconn_source", "VCONN source", T),
            ("preferred_role", "Preferred role", T),
            ("usb_typec_revision", "Type-C revision", T),
            ("usb_power_delivery_revision", "USB PD revision", T),
            ("usb_capability", "Supported USB modes", S),
        ],
        Some(Object::Partner) => &[
            ("type", "Partner product type", T),
            ("accessory_mode", "Accessory mode", T),
            (
                "supports_usb_power_delivery",
                "USB Power Delivery support",
                T,
            ),
            ("usb_power_delivery_revision", "USB PD revision", T),
            (
                "number_of_alternate_modes",
                "Advertised alternate-mode count",
                U,
            ),
            ("usb_mode", "Partner USB mode", S),
        ],
        Some(Object::Cable) => &[
            ("type", "Cable product type", T),
            ("plug_type", "Cable plug type", T),
            ("usb_power_delivery_revision", "USB PD revision", T),
        ],
        Some(Object::Plug) => &[(
            "number_of_alternate_modes",
            "Advertised alternate-mode count",
            U,
        )],
        Some(Object::AltMode) => &[
            ("svid", "Alternate-mode SVID", T),
            ("mode", "Alternate-mode index", U),
            ("vdo", "Alternate-mode VDO", T),
            ("description", "Alternate-mode description", T),
            (
                "active",
                "Alternate mode active",
                Format::Boolean("Active", "Inactive"),
            ),
            ("supported_roles", "Alternate-mode supported roles", T),
        ],
        Some(Object::Domain) => &[
            ("security", "Domain security policy", T),
            (
                "iommu_dma_protection",
                "IOMMU DMA protection",
                Format::Boolean("Enabled", "Disabled"),
            ),
            (
                "deauthorization",
                "Deauthorization supported",
                Format::Boolean("Yes", "No"),
            ),
        ],
        Some(Object::Router) => &[
            ("device_name", "Router device name", T),
            ("vendor_name", "Router vendor name", T),
            ("device", "Router device ID", T),
            ("vendor", "Router vendor ID", T),
            ("unique_id", "Router unique ID", T),
            ("generation", "Controller generation", U),
            (
                "authorized",
                "PCIe authorization",
                Format::Boolean("Authorized", "Unauthorized"),
            ),
            ("boot", "Authorized at boot", Format::Boolean("Yes", "No")),
            ("rx_speed", "RX speed per lane", T),
            ("tx_speed", "TX speed per lane", T),
            ("rx_lanes", "RX lane count", U),
            ("tx_lanes", "TX lane count", U),
        ],
        Some(Object::Usb4Port) => &[("link", "USB4 port link mode", T)],
        None => &[],
    };
    let mut rules = super::attributes::materialize_specs(schema);
    // Cached Discover Identity results, never issue a discovery command.
    if matches!(object(path), Some(Object::Partner | Object::Cable)) {
        for name in [
            "id_header",
            "cert_stat",
            "product",
            "product_type_vdo1",
            "product_type_vdo2",
            "product_type_vdo3",
        ] {
            rules.push(Rule::new(
                &format!("identity/{name}"),
                &format!("PD identity {name}"),
                T,
            ));
        }
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
    let thunderbolt = matches!(kind, Object::Domain | Object::Router | Object::Usb4Port);
    document.title = if thunderbolt {
        "Thunderbolt/USB4 connections"
    } else {
        "USB Type-C connections"
    }
    .into();
    for rule in rules(root) {
        reads.observe(document, root, &rule);
        checkpoint(document);
    }
    let name = root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut topology = Topology::new("Connection topology", name);
    let parents = buses::ancestors(root, 16, |path| object(path).is_some());
    let parent = buses::ancestor_chain(&mut topology, document, parents, None);
    let anchor = buses::add(
        &mut topology,
        document,
        root.into(),
        format!("{kind:?}"),
        parent,
    );
    let (children, coverage) =
        buses::select_descendants(root, 3, 24, |path| path != root && object(path).is_some());
    topology.coverage = coverage;
    for (index, child) in children.into_iter().enumerate() {
        if index < 8 {
            for rule in rules(&child).into_iter().filter(|rule| {
                matches!(
                    rule.name.as_str(),
                    "description"
                        | "active"
                        | "svid"
                        | "type"
                        | "supports_usb_power_delivery"
                        | "link"
                        | "authorized"
                        | "device_name"
                )
            }) {
                reads.observe(document, &child, &rule);
            }
            checkpoint(document);
        }
        buses::add_descendant(&mut topology, document, child, anchor);
    }
    buses::link_relations(
        document,
        root,
        &[
            "connector",
            "usb4_port",
            "device",
            "firmware_node",
            "driver",
        ],
        "Connection",
    );
    if let Some(driver) = buses::driver(root) {
        document
            .facts
            .push(Fact::text("Connection driver", driver, root.join("driver")));
    }
    document.facts.push(Fact::text("Related connection metadata scope", "Details requested for at most eight registered descendants; remaining objects are navigation links", root.into()));
    summarize(document, root, kind, &mut topology);
    document.facts.push(Fact::text("Connection observation policy", "Registered kernel objects and documented metadata only; no role swaps, alternate-mode activation, authorization, secure keys, firmware reads/updates or rescans", root.into()));
    document.topology = Some(topology);
    checkpoint(document);
}
fn summarize(document: &mut Document, root: &Path, kind: Object, topology: &mut Topology) {
    let current = document
        .facts
        .iter()
        .filter(|fact| fact.outcome == Outcome::Observed && !fact.retained);
    for label in [
        "Router device name",
        "Router vendor name",
        "Power role",
        "Data role",
        "Orientation",
        "Power operation mode",
        "Alternate-mode description",
        "Alternate mode active",
        "Domain security policy",
        "PCIe authorization",
        "USB4 port link mode",
        "Controller generation",
        "USB PD revision",
        "Partner product type",
        "Cable product type",
    ] {
        if let Some(fact) = current
            .clone()
            .find(|fact| fact.label == label && fact.source.parent() == Some(root))
        {
            if let Some(value) = &fact.value {
                topology.summary.push_str(&format!(" · {value}"));
            }
        }
    }
    if kind == Object::Port {
        for fact in current.filter(|fact| {
            fact.source != root.join("description") && fact.label == "Alternate-mode description"
        }) {
            if let Some(value) = &fact.value {
                topology.summary.push_str(&format!(" · {value}"));
            }
        }
        let partner = topology
            .nodes
            .iter()
            .any(|node| object(&node.path) == Some(Object::Partner));
        let value = if partner {
            "Partner registration observed"
        } else if topology.coverage == Outcome::Observed {
            "No partner registration observed; physical connection not independently verified"
        } else {
            "Partner inventory incomplete"
        };
        document
            .facts
            .push(Fact::text("Type-C partner observation", value, root.into()));
        topology.summary.push_str(if partner {
            " · Partner registered"
        } else if topology.coverage == Outcome::Observed {
            " · Partner not observed"
        } else {
            " · Partner inventory incomplete"
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, os::unix::fs::symlink, path::PathBuf};

    fn registered(base: &Path, parent: &Path, name: &str, subsystem: &str) -> PathBuf {
        let root = parent.join(name);
        let bus = base.join("subsystems").join(subsystem);
        std::fs::create_dir_all(&bus).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        symlink(bus, root.join("subsystem")).unwrap();
        root
    }
    fn document() -> Document {
        super::super::worker::interface(Path::new("/proc/uptime")).unwrap()
    }
    #[test]
    fn typec_port_partner_cable_and_alt_modes_keep_distinct_scopes() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        let port = registered(base, base, "port0", "typec");
        let partner = registered(base, &port, "port0-partner", "typec");
        let cable = registered(base, &port, "port0-cable", "typec");
        let plug = registered(base, &cable, "port0-cable-plug0", "typec");
        let mode = registered(base, &partner, "port0-partner.0", "typec");
        let mut reads = Reads::fixture(BTreeMap::from([
            (port.join("power_role"), b"[source] sink".to_vec()),
            (port.join("data_role"), b"[host] device".to_vec()),
            (port.join("orientation"), b"reverse".to_vec()),
            (partner.join("supports_usb_power_delivery"), b"yes".to_vec()),
            (mode.join("active"), b"1".to_vec()),
            (mode.join("description"), b"DisplayPort".to_vec()),
        ]));
        let mut doc = document();
        collect(&mut doc, &port, &mut reads, &mut |_| {});
        let graph = doc.topology.as_ref().unwrap();
        assert!(graph.summary.contains("source"));
        assert!(graph.summary.contains("Partner registered"));
        assert_eq!(graph.nodes.len(), 5);
        let plug_node = graph.nodes.iter().find(|node| node.path == plug).unwrap();
        assert_eq!(graph.nodes[plug_node.parent.unwrap()].path, cable);
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.source == mode.join("description")
                    && fact.value.as_deref() == Some("DisplayPort"))
        );
        assert!(
            rules(&port)
                .iter()
                .all(|rule| rule.name != "supports_usb_power_delivery")
        );
        assert!(rules(&partner).iter().all(|rule| rule.name != "power_role"));
        let nested = partner.join("identity");
        std::fs::create_dir(&nested).unwrap();
        assert!(rules(&nested).is_empty());
        assert!(reads.attempts().all(|path| !matches!(
            path.file_name().and_then(|n| n.to_str()),
            Some("key" | "nvm_version" | "rescan" | "offline")
        )));
    }
    #[test]
    fn compact_connector_summary_does_not_present_failed_or_retained_values_as_current() {
        let tmp = tempfile::tempdir().unwrap();
        let port = registered(tmp.path(), tmp.path(), "port0", "typec");
        let mut doc = document();
        doc.facts.clear();
        let mut old = Fact::text("Power role", "old-source", port.join("power_role"));
        old.retained = true;
        doc.facts.push(old);
        let mut failed = Fact::text("Data role", "old-host", port.join("data_role"));
        failed.outcome = Outcome::PermissionDenied;
        doc.facts.push(failed);
        doc.facts.push(Fact::text(
            "Orientation",
            "reverse",
            port.join("orientation"),
        ));
        let mut mode = Fact::text(
            "Alternate-mode description",
            "OldDisplayPort",
            port.join("port0.0/description"),
        );
        mode.retained = true;
        doc.facts.push(mode);
        let mut topology = Topology::new("Type-C", "port0".into());
        summarize(&mut doc, &port, Object::Port, &mut topology);
        assert!(topology.summary.contains("reverse"));
        for stale in ["old-source", "old-host", "OldDisplayPort"] {
            assert!(!topology.summary.contains(stale));
        }
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.value.as_deref() == Some("old-source") && fact.retained)
        );
    }
    #[test]
    fn cached_pd_identity_retains_raw_values_without_inventing_product_identity() {
        let temp = tempfile::tempdir().unwrap();
        let partner = registered(temp.path(), temp.path(), "port7-partner", "typec");
        let mut reads = Reads::fixture(BTreeMap::from([
            (partner.join("identity/id_header"), b"0x00000000".to_vec()),
            (partner.join("identity/product"), b"0x12340001".to_vec()),
            (partner.join("supports_usb_power_delivery"), b"no".to_vec()),
        ]));
        let mut doc = document();
        collect(&mut doc, &partner, &mut reads, &mut |_| {});
        assert_eq!(
            doc.observed_raw("PD identity id_header"),
            Some("0x00000000")
        );
        assert_eq!(doc.observed_raw("USB Power Delivery support"), Some("no"));
        assert!(
            doc.facts
                .iter()
                .all(|fact| !fact.label.contains("Physical identity"))
        );
    }
    #[test]
    fn thunderbolt_domain_router_and_usb4_links_exclude_authorization_controls() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        let domain = registered(base, base, "domain0", "thunderbolt");
        let host = registered(base, &domain, "0-0", "thunderbolt");
        let router = registered(base, &host, "0-1", "thunderbolt");
        let usb4 = registered(base, &router, "usb4_port1", "thunderbolt");
        let connector = registered(base, base, "port0", "typec");
        symlink(&connector, usb4.join("connector")).unwrap();
        let mut reads = Reads::fixture(BTreeMap::from([
            (router.join("authorized"), b"0".to_vec()),
            (router.join("device_name"), b"Dock".to_vec()),
            (router.join("generation"), b"4".to_vec()),
            (usb4.join("link"), b"usb4".to_vec()),
        ]));
        let mut doc = document();
        collect(&mut doc, &router, &mut reads, &mut |_| {});
        let graph = doc.topology.as_ref().unwrap();
        assert_eq!(graph.nodes[0].path, domain);
        assert_eq!(graph.nodes[1].path, host);
        assert!(graph.summary.contains("Unauthorized"));
        assert!(reads.attempts().all(|path| !matches!(
            path.file_name().and_then(|n| n.to_str()),
            Some("key" | "boot_acl" | "nvm_version" | "nvm_authenticate" | "rescan" | "offline")
        )));
        let mut doc = document();
        collect(&mut doc, &usb4, &mut reads, &mut |_| {});
        assert!(
            doc.relations
                .iter()
                .any(|relation| relation.target == connector)
        );
        assert!(rules(&domain).iter().all(|rule| rule.name != "authorized"));
    }
    #[test]
    fn unknown_objects_and_large_inventories_do_not_claim_complete_connections() {
        let temp = tempfile::tempdir().unwrap();
        let port = registered(temp.path(), temp.path(), "port1", "typec");
        for i in 0..30 {
            registered(temp.path(), &port, &format!("port1.{i}"), "typec");
        }
        let mut reads = Reads::fixture(BTreeMap::new());
        let mut doc = document();
        collect(&mut doc, &port, &mut reads, &mut |_| {});
        assert_eq!(doc.topology.as_ref().unwrap().coverage, Outcome::Truncated);
        assert_eq!(
            doc.observed_raw("Type-C partner observation"),
            Some("Partner inventory incomplete")
        );
        assert_eq!(doc.topology.as_ref().unwrap().nodes.len(), 25);
        for name in ["port", "portx", "port0-weird", "port0-partner.abc"] {
            let path = registered(temp.path(), temp.path(), name, "typec");
            assert!(rules(&path).is_empty());
        }
        let path = registered(temp.path(), temp.path(), "0-1.0", "thunderbolt");
        assert!(rules(&path).is_empty());
    }
}
