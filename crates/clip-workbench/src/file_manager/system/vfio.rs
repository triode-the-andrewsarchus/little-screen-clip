//! Passive VFIO/IOMMU registration and membership; never opens an access node.
use super::{
    Document, Relation,
    attributes::{self, Context},
    buses::{self, Topology},
    collector::Reads,
    model::Fact,
};
use std::path::Path;

#[derive(Clone, Copy)]
enum Interface {
    Group,
    Device,
    Control(bool),
}
impl Interface {
    fn title(self) -> &'static str {
        match self {
            Self::Group => "VFIO/IOMMU group",
            Self::Device => "VFIO device access interface",
            Self::Control(false) => "VFIO control interface",
            Self::Control(true) => "IOMMUFD control interface",
        }
    }
}

fn physical_parent(sys: &Path) -> Option<&Path> {
    sys.ancestors()
        .skip(1)
        .take(4)
        .find(|path| path.join("iommu_group").exists())
}

fn registration(sys: &Path, groups: &Path) -> Option<(Interface, Option<std::path::PathBuf>)> {
    let name = sys.file_name()?.to_str()?;
    let subsystem = buses::subsystem(sys);
    let role = if (sys.parent() == Some(groups) || subsystem.as_deref() == Some("vfio"))
        && number(name).is_some()
    {
        Interface::Group
    } else if subsystem.as_deref() == Some("vfio-dev") && buses::numbered(name, "vfio") {
        Interface::Device
    } else if subsystem.as_deref() == Some("misc") && matches!(name, "vfio" | "iommu") {
        Interface::Control(name == "iommu")
    } else {
        return None;
    };
    let group = match role {
        Interface::Group => Some(if sys.parent() == Some(groups) {
            sys.into()
        } else {
            groups.join(name)
        }),
        Interface::Device => {
            physical_parent(sys).and_then(|parent| parent.join("iommu_group").canonicalize().ok())
        }
        Interface::Control(_) => None,
    };
    Some((role, group))
}

pub(super) fn directory_group(sys: &Path) -> Option<(Option<std::path::PathBuf>, String)> {
    let (role, group) = registration(sys, Path::new("/sys/kernel/iommu_groups"))?;
    Some((group, role.title().into()))
}

fn number(name: &str) -> Option<u32> {
    (!name.is_empty() && name.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| name.parse().ok())
        .flatten()
}

pub(super) fn unmapped(document: &mut Document) {
    // A stale conventional node is not evidence that its kernel interface exists.
    if document.path == Path::new("/dev/vfio/vfio")
        && matches!(
            document.kind,
            little_clip_core::kernel_interface::Kind::Character {
                major: 10,
                minor: 196
            }
        )
        && !document
            .relations
            .iter()
            .any(|relation| relation.label == "Kernel object")
    {
        document.title = "VFIO control path (registration unavailable)".into();
        let mut fact = Fact::text(
            "VFIO registration",
            "Conventional control node exists, but no matching kernel object is exposed; availability and container state unverified",
            document.path.clone(),
        );
        fact.interface = true;
        document.facts.insert(0, fact);
    }
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
        Path::new("/sys/kernel/iommu_groups"),
        reads,
        checkpoint,
    );
}

fn collect_at(
    document: &mut Document,
    sys: &Path,
    groups: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let Some((role, selected)) = registration(sys, groups) else {
        return;
    };
    document.title = role.title().into();
    match role {
        Interface::Device => {
            if let Some(parent) = physical_parent(sys) {
                document
                    .relations
                    .push(Relation::new("VFIO underlying device", parent.into()));
            } else {
                document.facts.insert(
                    0,
                    Fact::text(
                        "VFIO device",
                        "Underlying IOMMU group not exposed",
                        sys.into(),
                    ),
                );
            }
        }
        Interface::Control(_) => {
            document.facts.insert(
            0,
            Fact::text(
                "Control interface",
                "No group, device assignment or container state inferred; access node not opened",
                sys.into(),
            ),
        );
        }
        Interface::Group => {}
    }
    if let Some(selected) = selected {
        document.topology = Some(group(document, &selected, reads, checkpoint));
    }
    checkpoint(document);
}

/// Shared by PCI, sysfs groups, legacy VFIO groups and device-centric access.
pub(super) fn group(
    document: &mut Document,
    path: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) -> Topology {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    document.facts.push(Fact::text(
        "IOMMU group number",
        name.to_string(),
        path.into(),
    ));
    let mut topology = Topology::new("IOMMU group membership", format!("IOMMU group {name}"));
    let parent = buses::add(
        &mut topology,
        document,
        path.into(),
        format!("IOMMU group {name}"),
        None,
    );
    members_into(document, path, reads, checkpoint, &mut topology, parent);
    topology
}

/// Shares bounded membership acquisition with directory inventories.
pub(super) fn members_into(
    document: &mut Document,
    path: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
    topology: &mut Topology,
    parent: usize,
) {
    let entries = match std::fs::read_dir(path.join("devices")) {
        Ok(entries) => entries,
        Err(error) => {
            let fact = Fact::failure("IOMMU membership", path.join("devices"), error);
            if topology.coverage == super::model::Outcome::Observed {
                topology.coverage = fact.outcome.clone();
            }
            document.facts.push(fact);
            return;
        }
    };
    let (paths, coverage) = buses::select_children(entries, |_| true, 32);
    if coverage != super::model::Outcome::Observed {
        topology.coverage = coverage;
    }
    let mut members = Vec::new();
    for link in paths {
        match link.canonicalize() {
            Ok(target) => {
                let label = member(document, &target, reads);
                buses::add(
                    topology,
                    document,
                    target.clone(),
                    label.clone(),
                    Some(parent),
                );
                buses::access_node(document, &target, "Group member access interface");
                members.push(label);
            }
            Err(error) => {
                let fact = Fact::failure("IOMMU member", link, error);
                if topology.coverage == super::model::Outcome::Observed {
                    topology.coverage = fact.outcome.clone();
                }
                document.facts.push(fact);
            }
        }
        checkpoint(document);
    }
    topology
        .summary
        .push_str(&format!(" · {} observed members", members.len()));
    for label in members.iter().take(3) {
        topology.summary.push_str(&format!(" · {label}"));
    }
    let mut fact = Fact::text(
        "IOMMU membership",
        format!(
            "{} observed members; group membership and vfio-pci binding do not prove VM assignment or exclusive ownership",
            members.len()
        ),
        path.into(),
    );
    fact.outcome = topology.coverage.clone();
    document.facts.insert(0, fact);
}

fn member(document: &mut Document, path: &Path, reads: &mut Reads) -> String {
    let address = path.file_name().unwrap_or_default().to_string_lossy();
    let driver = buses::driver(path);
    let mut label = address.to_string();
    if buses::subsystem(path).as_deref() == Some("pci") {
        let context = Context::discover(path);
        for name in ["vendor", "device", "class"] {
            if let Some(rule) = attributes::rule(&context, name) {
                let mut fact = reads.fact(path, &rule);
                fact.label = format!("{address} · {}", fact.label);
                if let Some(value) = &fact.value {
                    label.push_str(&format!(" · {value}"));
                }
                document.facts.push(fact);
            }
        }
        if let Ok(device) = udev::Device::from_syspath(path) {
            for property in ["ID_VENDOR_FROM_DATABASE", "ID_MODEL_FROM_DATABASE"] {
                if let Some(mut fact) =
                    buses::database_fact(&device, path, property, "Device database")
                {
                    if let Some(value) = &fact.value {
                        label.push_str(&format!(" · {value}"));
                    }
                    fact.label = format!("{address} · {}", fact.label);
                    document.facts.push(fact);
                }
            }
        }
    }
    label.push_str(&format!(
        " · {}",
        driver.as_deref().unwrap_or("binding unavailable")
    ));
    document.facts.push(Fact::text(
        format!("{address} · Driver binding"),
        driver.as_deref().unwrap_or("not exposed"),
        path.join("driver"),
    ));
    if let Ok(driver) = path.join("driver").canonicalize() {
        document
            .relations
            .push(Relation::new(format!("{address} driver"), driver));
    }
    // No false negative when the binding cannot be observed.
    let binding = match driver.as_deref() {
        Some("vfio-pci") => "Bound to vfio-pci; assignment unknown",
        Some(_) => "Observed other driver; assignment unknown",
        None => "Binding unavailable; assignment unknown",
    };
    document.facts.push(Fact::text(
        format!("{address} · VFIO binding"),
        binding,
        path.into(),
    ));
    label
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, os::unix::fs::symlink};

    fn document(path: &Path) -> Document {
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        document.path = path.into();
        document.facts.clear();
        document.relations.clear();
        document
    }

    #[test]
    fn group_membership_preserves_pci_identity_and_binding_without_device_content() {
        let temp = tempfile::tempdir().unwrap();
        let group = temp.path().join("iommu_groups/12");
        let pci = temp.path().join("pci");
        let driver = temp.path().join("drivers/vfio-pci");
        let device = temp.path().join("0000:01:00.0");
        for path in [
            group.join("devices"),
            pci.clone(),
            driver.clone(),
            device.clone(),
        ] {
            std::fs::create_dir_all(path).unwrap();
        }
        symlink(&pci, device.join("subsystem")).unwrap();
        symlink(&driver, device.join("driver")).unwrap();
        symlink(&group, device.join("iommu_group")).unwrap();
        symlink(&device, group.join("devices/0000:01:00.0")).unwrap();
        let mut reads = Reads::fixture(BTreeMap::from([
            (device.join("vendor"), b"0x10de".to_vec()),
            (device.join("device"), b"0x2204".to_vec()),
            (device.join("class"), b"0x030000".to_vec()),
        ]));
        let mut document = document(&group);
        collect_at(
            &mut document,
            &group,
            group.parent().unwrap(),
            &mut reads,
            &mut |_| {},
        );
        let topology = document.topology.as_ref().unwrap();
        assert_eq!(topology.nodes.len(), 2);
        assert!(topology.summary.contains("0x10de"));
        assert!(topology.summary.contains("vfio-pci"));
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.value.as_deref() == Some("Bound to vfio-pci; assignment unknown"))
        );
        assert!(
            document
                .relations
                .iter()
                .any(|relation| relation.target == driver)
        );
        assert!(reads.attempts().all(|path| matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("vendor" | "device" | "class")
        )));
        let mut pci_document = document.clone();
        pci_document.topology = None;
        assert_eq!(
            super::super::pci::iommu(&mut pci_document, &device, &mut reads),
            Some(group)
        );
        assert!(
            pci_document
                .facts
                .iter()
                .any(|fact| fact.label == "IOMMU membership")
        );
    }

    #[test]
    fn legacy_group_registration_is_resolved_without_opening_its_access_node() {
        let temp = tempfile::tempdir().unwrap();
        let groups = temp.path().join("groups");
        let group = groups.join("12");
        let access = temp.path().join("access/12");
        let subsystem = temp.path().join("vfio");
        for path in [group.join("devices"), access.clone(), subsystem.clone()] {
            std::fs::create_dir_all(path).unwrap();
        }
        symlink(subsystem, access.join("subsystem")).unwrap();
        let mut reads = Reads::fixture(BTreeMap::new());
        let mut doc = document(&access);
        collect_at(&mut doc, &access, &groups, &mut reads, &mut |_| {});
        assert_eq!(doc.title, "VFIO/IOMMU group");
        assert!(
            doc.relations
                .iter()
                .any(|relation| relation.target == group)
        );
        assert_eq!(reads.attempts().count(), 0);
        let mut doc = document(Path::new("/dev/vfio/vfio"));
        doc.kind = little_clip_core::kernel_interface::Kind::Character {
            major: 10,
            minor: 196,
        };
        unmapped(&mut doc);
        assert!(doc.title.contains("registration unavailable"));
        assert!(doc.facts[0].interface);
        assert_eq!(number("-1"), None);
        assert_eq!(number("4294967296"), None);
        assert_eq!(number("12x"), None);
    }

    #[test]
    fn modern_device_and_control_interfaces_do_not_infer_container_state() {
        let temp = tempfile::tempdir().unwrap();
        let groups = temp.path().join("groups");
        let group = groups.join("8");
        let device = temp.path().join("device/vfio-dev/vfio3");
        let subsystem = temp.path().join("vfio-dev");
        let misc = temp.path().join("misc");
        let control = temp.path().join("iommu");
        for path in [
            group.join("devices"),
            device.clone(),
            subsystem.clone(),
            misc.clone(),
            control.clone(),
        ] {
            std::fs::create_dir_all(path).unwrap();
        }
        symlink(&subsystem, device.join("subsystem")).unwrap();
        symlink(&group, temp.path().join("device/iommu_group")).unwrap();
        symlink(&misc, control.join("subsystem")).unwrap();
        let mut reads = Reads::fixture(BTreeMap::new());
        let mut doc = document(&device);
        collect_at(&mut doc, &device, &groups, &mut reads, &mut |_| {});
        assert_eq!(doc.title, "VFIO device access interface");
        assert!(doc.topology.as_ref().unwrap().summary.contains("group 8"));
        let mut doc = document(&control);
        collect_at(&mut doc, &control, &groups, &mut reads, &mut |_| {});
        assert_eq!(doc.title, "IOMMUFD control interface");
        assert!(doc.topology.is_none());
        assert_eq!(reads.attempts().count(), 0);
    }

    #[test]
    fn missing_and_large_group_inventories_disclose_incomplete_observations() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("7");
        let mut reads = Reads::fixture(BTreeMap::new());
        let mut doc = document(&path);
        assert_eq!(
            group(&mut doc, &path, &mut reads, &mut |_| {}).coverage,
            super::super::model::Outcome::Missing
        );
        std::fs::create_dir_all(path.join("devices")).unwrap();
        for id in 0..40 {
            let device = temp.path().join(format!("member{id:02}"));
            std::fs::create_dir(&device).unwrap();
            symlink(device, path.join(format!("devices/{id:02}"))).unwrap();
        }
        let topology = group(&mut doc, &path, &mut reads, &mut |_| {});
        assert_eq!(topology.coverage, super::super::model::Outcome::Truncated);
        assert_eq!(topology.nodes.len(), 33);
        assert!(
            doc.facts.iter().any(
                |fact| fact.value.as_deref() == Some("Binding unavailable; assignment unknown")
            )
        );
        assert_eq!(reads.attempts().count(), 0);
    }
}
