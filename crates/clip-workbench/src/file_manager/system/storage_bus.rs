//! Registered storage/PHY metadata; no scan, reset, enclosure or register I/O.
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
    Scsi,
    AtaPort,
    AtaLink,
    AtaDevice,
    Mdio(bool),
}

fn dotted(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix).is_some_and(|n| {
        !n.is_empty()
            && n.split('.').all(|part| {
                !part.is_empty()
                    && part.bytes().all(|b| b.is_ascii_digit())
                    && part.parse::<u32>().is_ok()
            })
    })
}
fn object(path: &Path) -> Option<Object> {
    let name = path.file_name()?.to_str()?;
    match buses::subsystem(path).as_deref()? {
        "scsi_host" if buses::numbered(name, "host") => Some(Object::Scsi),
        "ata_port" if buses::numbered(name, "ata") => Some(Object::AtaPort),
        "ata_link" if dotted(name, "link") => Some(Object::AtaLink),
        "ata_device" if dotted(name, "dev") => Some(Object::AtaDevice),
        "mdio_bus" => Some(Object::Mdio(path.join("phy_id").is_file())),
        _ => None,
    }
}
pub(super) fn rules(path: &Path) -> Vec<Rule> {
    use Format::{Text as T, Unsigned as U};
    let specs: &[(&str, &str, Format)] = match object(path) {
        Some(Object::Scsi) => &[
            ("proc_name", "SCSI host driver label", T),
            ("state", "SCSI host kernel state", T),
            ("unique_id", "Driver-provided host identifier", U),
            ("can_queue", "Host queue capacity", U),
            ("cmd_per_lun", "Commands per LUN limit", Format::Integer),
            ("sg_tablesize", "Scatter/gather entry capacity", U),
            (
                "link_power_management_policy",
                "Configured link power policy",
                T,
            ),
        ],
        Some(Object::AtaPort) => &[
            ("port_no", "Host-local ATA port number", U),
            ("nr_pmp_links", "Registered port-multiplier link count", U),
            ("idle_irq", "Idle interrupt count", U),
        ],
        Some(Object::AtaLink) => &[
            ("hw_sata_spd_limit", "Exposed hardware SATA speed limit", T),
            ("sata_spd_limit", "Libata SATA speed limit", T),
            ("sata_spd", "Reported SATA link speed", T),
        ],
        Some(Object::AtaDevice) => &[
            ("class", "ATA device class", T),
            ("pio_mode", "Configured PIO mode", T),
            ("dma_mode", "Configured DMA mode", T),
            ("xfer_mode", "Configured transfer mode", T),
            ("trim", "Reported TRIM mode", T),
            ("spdn_cnt", "Recorded speed-down count", U),
        ],
        Some(Object::Mdio(true)) => &[
            ("phy_id", "Cached PHY identifier", T),
            ("phy_interface", "Configured PHY interface", T),
            (
                "phy_has_fixups",
                "Kernel PHY fixups applied",
                Format::Boolean("Yes", "No"),
            ),
            (
                "phy_standalone",
                "Reported standalone PHY mode",
                Format::Boolean("Yes", "No"),
            ),
            (
                "phy_dev_flags",
                "PHY consumer flags (kernel-specific hex)",
                T,
            ),
        ],
        Some(Object::Mdio(false)) | None => &[],
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
        Object::Scsi => "SCSI host metadata",
        Object::AtaPort => "ATA port metadata",
        Object::AtaLink => "ATA link metadata",
        Object::AtaDevice => "ATA device metadata",
        Object::Mdio(true) => "Registered MDIO PHY metadata",
        Object::Mdio(false) => "Registered MDIO device metadata",
    };
    document.title = title.into();
    let summary = buses::attribute_summary(document, root);
    let mut topology = Topology::new("Registered controller/device relationships", summary);
    let ancestors = buses::ancestors(root, 12, |path| {
        object(path).is_some()
            || matches!(
                buses::subsystem(path).as_deref(),
                Some("pci" | "usb" | "platform" | "scsi" | "net")
            )
    });
    let parent = buses::ancestor_chain(&mut topology, document, ancestors, None);
    let anchor = buses::add(&mut topology, document, root.into(), title.into(), parent);
    let backing = root
        .join("device")
        .canonicalize()
        .ok()
        .filter(|path| path.is_dir());
    let scope = backing.as_deref().unwrap_or(root);
    let (children, coverage) = buses::select_descendants(scope, 4, 16, |path| {
        path != root
            && path != scope
            && (object(path).is_some()
                || matches!(
                    buses::subsystem(path).as_deref(),
                    Some("scsi" | "scsi_device" | "block" | "scsi_generic")
                ))
    });
    topology.coverage = coverage;
    for child in children {
        buses::add_descendant(&mut topology, document, child, anchor);
    }
    buses::link_relations(
        document,
        root,
        &[
            "device",
            "driver",
            "firmware_node",
            "of_node",
            "attached_dev",
        ],
        "Storage/PHY",
    );
    buses::access_node(document, root, "Storage/PHY access interface (unopened)");
    document.facts.push(Fact::text("Storage/PHY observation policy","Registered topology/cached or documented metadata only; host state, interface mode and registration do not prove disk health, carrier or negotiated Ethernet speed. No bus scans, resets, enclosure messages/buffers, disk/sg I/O, IDENTIFY payloads, PHY register/statistics queries or control changes",root.into()));
    document.topology = Some(topology);
    checkpoint(document);
}

#[cfg(test)]
mod tests {
    use super::super::collector::registered_fixture;
    use super::*;
    use std::os::unix::fs::symlink;
    #[test]
    fn scsi_host_metadata_excludes_scans_resets_and_enclosure_buffers() {
        let (_temp, root, mut reads, mut doc) = registered_fixture(
            "scsi_host",
            "host0",
            &[
                ("proc_name", "ahci"),
                ("state", "running"),
                ("can_queue", "32"),
                ("cmd_per_lun", "1"),
            ],
        );
        collect(&mut doc, &root, &mut reads, &mut |_| {});
        assert_eq!(doc.title, "SCSI host metadata");
        assert!(doc.topology.as_ref().unwrap().summary.contains("ahci"));
        assert!(reads.attempts().all(|path| !matches!(
            path.file_name().and_then(|n| n.to_str()),
            Some("scan" | "host_reset" | "em_message" | "em_buffer" | "ahci_port_cmd")
        )));
    }
    #[test]
    fn ata_port_link_and_device_have_distinct_schemas_and_no_identify_payload_reads() {
        for (subsystem, name, attribute, value) in [
            ("ata_port", "ata1", "port_no", "0"),
            ("ata_link", "link1.0", "sata_spd", "6.0 Gbps"),
            ("ata_device", "dev1.0", "xfer_mode", "UDMA/133"),
        ] {
            let (_temp, root, mut reads, mut doc) =
                registered_fixture(subsystem, name, &[(attribute, value)]);
            collect(&mut doc, &root, &mut reads, &mut |_| {});
            assert!(doc.topology.as_ref().unwrap().summary.contains(value));
            assert!(reads.attempts().all(|path| !matches!(
                path.file_name().and_then(|n| n.to_str()),
                Some("id" | "gscr" | "ering")
            )));
            let nested = root.join("private");
            std::fs::create_dir(&nested).unwrap();
            assert!(rules(&nested).is_empty());
        }
    }
    #[test]
    fn mdio_phy_identity_and_configured_interface_do_not_query_registers_or_infer_carrier() {
        let (temp, root, mut reads, mut doc) = registered_fixture(
            "mdio_bus",
            "mdio0:01",
            &[
                ("phy_id", "0x12345678"),
                ("phy_interface", "rgmii"),
                ("phy_has_fixups", "1"),
            ],
        );
        let network = temp.path().join("net0");
        std::fs::create_dir(&network).unwrap();
        symlink(&network, root.join("attached_dev")).unwrap();
        collect(&mut doc, &root, &mut reads, &mut |_| {});
        assert_eq!(doc.title, "Registered MDIO PHY metadata");
        assert_eq!(
            doc.observed_raw("Cached PHY identifier"),
            Some("0x12345678")
        );
        assert!(
            doc.relations
                .iter()
                .any(|relation| relation.target == network)
        );
        assert!(reads.attempts().all(|path| !matches!(
            path.file_name().and_then(|n| n.to_str()),
            Some("phy_statistics" | "registers" | "speed" | "carrier")
        )));
        let (_temp, root, mut reads, mut doc) = registered_fixture("mdio_bus", "mdio0:02", &[]);
        collect(&mut doc, &root, &mut reads, &mut |_| {});
        assert_eq!(reads.attempts().count(), 0);
        assert_eq!(doc.title, "Registered MDIO device metadata");
    }
    #[test]
    fn backing_device_topology_is_bounded_and_preserves_registered_descendants() {
        let (temp, root, mut reads, mut doc) = registered_fixture("ata_port", "ata1", &[]);
        let backing = temp.path().join("backing");
        std::fs::create_dir(&backing).unwrap();
        symlink(&backing, root.join("device")).unwrap();
        let class = temp.path().join("class/ata_link");
        std::fs::create_dir_all(&class).unwrap();
        for i in 0..20 {
            let node = backing.join(format!("link1.{i}"));
            std::fs::create_dir(&node).unwrap();
            symlink(&class, node.join("subsystem")).unwrap();
        }
        collect(&mut doc, &root, &mut reads, &mut |_| {});
        let data = doc.topology.as_ref().unwrap();
        assert_eq!(data.nodes.len(), 17);
        assert_eq!(data.coverage, super::super::model::Outcome::Truncated);
        assert!(
            reads
                .attempts()
                .all(|path| path.parent() == Some(root.as_path()))
        );
    }
}
