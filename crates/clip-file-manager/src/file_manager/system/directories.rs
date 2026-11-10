//! Identity-scoped sysfs directory summaries; bounded immediate metadata only.
use super::{
    Document, Relation,
    attributes::{Format, Rule},
    buses::{self, Topology},
    collector::Reads,
    model::{Fact, Outcome},
};
use little_clip_core::kernel_interface::{self, Kind};
use std::path::{Path, PathBuf};

/// Counts immediate, directly registered children without reading their attributes.
pub(super) fn child_devices(document: &mut Document, root: &Path) {
    if !document.interface_directory
        || document.kind
            != (Kind::Kernel {
                filesystem: kernel_interface::Filesystem::Sysfs,
            })
    {
        return;
    }
    let inventory = super::identity::MountIdentity::directory(root).and_then(|mount| {
        std::fs::read_dir(root).map(|entries| {
            let (paths, coverage) = buses::select_children(entries, |_| true, 128);
            (mount, paths, coverage)
        })
    });
    let (mount, paths, mut coverage) = match inventory {
        Ok(inventory) => inventory,
        Err(error) => {
            document
                .facts
                .push(Fact::failure("Child device inventory", root.into(), error));
            return;
        }
    };
    let mut groups = std::collections::BTreeMap::<String, usize>::new();
    let mut retained = 0;
    for path in paths {
        match child_registration(mount, &path) {
            Ok(Some(subsystem)) => {
                *groups.entry(subsystem).or_default() += 1;
                if retained < 16 {
                    document.relations.push(Relation::new("Child device", path));
                    retained += 1;
                }
            }
            Ok(None) => {}
            Err(error) => {
                if coverage != Outcome::Truncated {
                    coverage = Outcome::Unsupported;
                }
                document
                    .facts
                    .push(Fact::failure("Child registration", path, error));
            }
        }
    }
    let total: usize = groups.values().sum();
    let types = groups
        .iter()
        .map(|(name, count)| format!("{name}: {count}"))
        .collect::<Vec<_>>()
        .join(" · ");
    let mut fact = Fact::text(
        "Child device inventory",
        format!(
            "{total} directly registered children{} · {coverage} coverage; at most 128 immediate entries scanned, {retained} links retained; non-atomic registration snapshot, not a physical-device count",
            if types.is_empty() {
                String::new()
            } else {
                format!(" · {types}")
            }
        ),
        root.into(),
    );
    fact.outcome = coverage;
    document.facts.push(fact);
}

fn child_registration(
    mount: super::identity::MountIdentity,
    path: &Path,
) -> std::io::Result<Option<String>> {
    // Alias directories and attribute groups must not acquire a parent's registration.
    if !std::fs::symlink_metadata(path)?.is_dir() {
        return Ok(None);
    }
    let subsystem = match path.join("subsystem").canonicalize() {
        Ok(subsystem) => subsystem,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !mount.same_hierarchy_mount(super::identity::MountIdentity::directory(path)?)
        || !mount.same_hierarchy_mount(super::identity::MountIdentity::directory(&subsystem)?)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Child registration crosses the observed hierarchy mount",
        ));
    }
    Ok(subsystem
        .file_name()
        .map(|name| name.to_string_lossy().into_owned()))
}

#[derive(Clone, Copy, PartialEq)]
enum Role {
    Plain,
    Buses,
    Classes,
    Power,
    Mapping(bool),
}
const ROOTS: &[(&str, &str, &str, Role)] = &[
    (
        "devices",
        "Device hierarchy",
        "Kernel-exposed physical and virtual device hierarchy; directories are not a count of physical devices",
        Role::Plain,
    ),
    (
        "bus",
        "Bus registration inventory",
        "Available kernel bus types, registered device names and driver directories",
        Role::Buses,
    ),
    (
        "class",
        "Functional device classes",
        "Kernel device classes such as DRM, network, sound, thermal and hardware monitoring",
        Role::Classes,
    ),
    (
        "block",
        "Block-device inventory",
        "Kernel block-device objects; inspect individual objects for geometry and mount relationships",
        Role::Plain,
    ),
    (
        "dev",
        "Device-number lookup",
        "Character and block major/minor lookup directories; names link to their kernel objects",
        Role::Plain,
    ),
    (
        "dev/char",
        "Character device-number mappings",
        "Observed major/minor mappings and verified access-node references",
        Role::Mapping(false),
    ),
    (
        "dev/block",
        "Block device-number mappings",
        "Observed major/minor mappings and verified access-node references",
        Role::Mapping(true),
    ),
    (
        "module",
        "Kernel module inventory",
        "Exposed loaded or built-in module objects; directories alone do not distinguish those states",
        Role::Plain,
    ),
    (
        "fs",
        "Filesystem kernel interfaces",
        "Filesystem interfaces, including cgroups when mounted; this is not a complete mounted-filesystem inventory",
        Role::Plain,
    ),
    (
        "kernel",
        "Kernel subsystems and state",
        "Kernel subsystem interfaces and supported global state; unknown payloads remain unread",
        Role::Plain,
    ),
    (
        "power",
        "System sleep configuration",
        "Supported and configured sleep modes, not a promise that the next transition will succeed",
        Role::Power,
    ),
];
const POWER: &[(&str, &str, Format)] = &[
    ("state", "Supported system sleep states", Format::Text),
    ("mem_sleep", "Configured suspend mode", Format::Selection),
    ("disk", "Configured hibernation method", Format::Selection),
];
pub(super) fn collect(
    document: &mut Document,
    root: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) -> bool {
    collect_at(document, root, Path::new("/sys"), reads, checkpoint)
}
fn collect_at(
    document: &mut Document,
    root: &Path,
    sysfs: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) -> bool {
    let Some((_, title, description, role)) = ROOTS
        .iter()
        .find(|(name, _, _, _)| super::boot_firmware::same_object(root, &sysfs.join(name)))
    else {
        return false;
    };
    let requested = document.requested_attribute();
    if requested.is_some() && *role != Role::Power {
        return false;
    }
    if requested
        .as_deref()
        .is_some_and(|name| !POWER.iter().any(|(field, _, _)| name == *field))
    {
        return false;
    }
    document.title = (*title).into();
    document
        .facts
        .push(Fact::text("Directory purpose", *description, root.into()));
    checkpoint(document);
    if *role == Role::Power {
        for (name, label, format) in POWER
            .iter()
            .filter(|(name, _, _)| requested.as_deref().is_none_or(|request| request == *name))
        {
            document
                .facts
                .push(reads.fact(root, &Rule::new(name, label, *format)));
            checkpoint(document);
        }
    }
    if document.interface_directory {
        inventory(document, root, title, *role, checkpoint);
    }
    document
        .facts
        .sort_by_key(|fact| (fact.interface, fact.label == "Directory purpose"));
    checkpoint(document);
    true
}
fn children(root: &Path, maximum: usize) -> std::io::Result<(Vec<PathBuf>, Outcome)> {
    Ok(buses::select_children(
        std::fs::read_dir(root)?,
        |entry| {
            entry
                .file_type()
                .is_ok_and(|kind| kind.is_dir() || kind.is_symlink())
        },
        maximum,
    ))
}
fn inventory(
    document: &mut Document,
    root: &Path,
    title: &str,
    role: Role,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let mut topology = Topology::new(title, title.into());
    buses::add(&mut topology, document, root.into(), title.into(), None);
    match children(root, 128) {
        Ok((mut paths, mut coverage)) => {
            let priority: &[&str] = match role {
                Role::Classes => &["drm", "net", "sound", "thermal", "hwmon", "power_supply"],
                Role::Buses => &["usb", "pci", "platform", "i2c", "spi", "nvme"],
                _ => &[],
            };
            paths.sort_by_key(|path| {
                (
                    priority
                        .iter()
                        .position(|name| path.file_name().is_some_and(|leaf| leaf == *name))
                        .unwrap_or(usize::MAX),
                    path.clone(),
                )
            });
            if paths.len() > 16 {
                paths.truncate(16);
                coverage = Outcome::Truncated;
            }
            topology.coverage = coverage;
            for path in paths {
                buses::add_descendant(&mut topology, document, path.clone(), 0);
                match role {
                    Role::Buses => {
                        count(
                            document,
                            &path,
                            &path.join("devices"),
                            "Registered device names",
                        );
                        count(document, &path, &path.join("drivers"), "Driver directories");
                    }
                    Role::Classes => count(document, &path, &path, "Class member names"),
                    Role::Mapping(block) => access_node(document, &path, block),
                    _ => {}
                }
                checkpoint(document);
            }
        }
        Err(error) => {
            topology.coverage = Outcome::Unsupported;
            document
                .facts
                .push(Fact::failure("Directory inventory", root.into(), error));
        }
    }
    let retained = topology.nodes.len().saturating_sub(1);
    let mut fact = Fact::text(
        "Inventory coverage",
        format!(
            "{retained} immediate entries retained; {} coverage; at most 128 entries scanned and 16 retained. Counts describe observed registration names, not physical devices; no recursive traversal",
            topology.coverage
        ),
        root.into(),
    );
    fact.outcome = topology.coverage.clone();
    document.facts.push(fact);
    topology.summary = format!("{title} · {retained} retained · {}", topology.coverage);
    if role == Role::Power {
        topology.summary = buses::attribute_summary(document, root);
    }
    document.topology = Some(topology);
}
fn count(document: &mut Document, context: &Path, root: &Path, label: &str) {
    let context = context.file_name().unwrap_or_default().to_string_lossy();
    let label = format!("{context} · {label}");
    let fact = match children(root, 128) {
        Ok((paths, coverage)) => {
            let mut fact = Fact::text(
                label,
                format!(
                    "{} observed names · {} coverage · maximum 128 entries scanned; not an atomic snapshot",
                    paths.len(),
                    coverage
                ),
                root.into(),
            );
            fact.outcome = coverage;
            fact
        }
        Err(error) => Fact::failure(label, root.into(), error),
    };
    document.facts.push(fact);
}
fn access_node(document: &mut Document, path: &Path, block: bool) {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let Some((major, minor)) = name
        .split_once(':')
        .and_then(|(major, minor)| Some((major.parse::<u32>().ok()?, minor.parse::<u32>().ok()?)))
    else {
        return;
    };
    let Ok(canonical) = path.canonicalize() else {
        return;
    };
    let Ok(device) = udev::Device::from_syspath(&canonical) else {
        return;
    };
    let Some(node) = device.devnode() else { return };
    let expected = if block {
        Kind::Block { major, minor }
    } else {
        Kind::Character { major, minor }
    };
    if kernel_interface::classify(node).ok() == Some(expected) {
        document.relations.push(Relation::new(
            format!("Verified access node {name}"),
            node.into(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::kernel_interface::Filesystem;

    #[test]
    fn generic_children_require_direct_registration_and_exclude_aliases_and_attribute_groups() {
        use std::os::unix::fs::symlink;
        let (temp, root, reads, mut document) = fixture("devices", &[]);
        for (name, subsystem) in [("tty0", "tty"), ("tty1", "tty"), ("input0", "input")] {
            let target = temp.path().join("class").join(subsystem);
            std::fs::create_dir_all(&target).unwrap();
            let child = root.join(name);
            std::fs::create_dir(&child).unwrap();
            symlink(target, child.join("subsystem")).unwrap();
            std::fs::write(child.join("payload"), b"must not be read").unwrap();
        }
        std::fs::create_dir(root.join("power")).unwrap();
        std::fs::create_dir(root.join("power/nested-device")).unwrap();
        symlink(
            temp.path().join("class/tty"),
            root.join("power/nested-device/subsystem"),
        )
        .unwrap();
        symlink(root.join("tty0"), root.join("alias")).unwrap();
        child_devices(&mut document, &root);
        let inventory = document
            .facts
            .iter()
            .find(|fact| fact.label == "Child device inventory")
            .unwrap();
        assert_eq!(inventory.outcome, Outcome::Observed);
        let value = inventory.value.as_ref().unwrap();
        assert!(value.contains("3 directly registered children"));
        assert!(value.contains("input: 1 · tty: 2"));
        assert_eq!(
            document
                .relations
                .iter()
                .filter(|relation| relation.label == "Child device")
                .count(),
            3
        );
        assert_eq!(reads.attempts().count(), 0);
    }

    #[test]
    fn generic_child_inventory_discloses_caps_and_failures_and_rejects_other_contexts() {
        use std::os::unix::fs::symlink;
        let (temp, root, _, mut document) = fixture("devices", &[]);
        let class = temp.path().join("class/fixture");
        for index in 0..140 {
            let child = root.join(format!("child{index:03}"));
            std::fs::create_dir(&child).unwrap();
            symlink(&class, child.join("subsystem")).unwrap();
        }
        child_devices(&mut document, &root);
        let inventory = document
            .facts
            .iter()
            .find(|fact| fact.label == "Child device inventory")
            .unwrap();
        assert_eq!(inventory.outcome, Outcome::Truncated);
        assert_eq!(
            document
                .relations
                .iter()
                .filter(|relation| relation.label == "Child device")
                .count(),
            16
        );
        document.facts.clear();
        child_devices(&mut document, &root.join("missing"));
        assert_eq!(document.facts[0].outcome, Outcome::Missing);
        document.facts.clear();
        let errors = temp.path().join("errors");
        std::fs::create_dir(&errors).unwrap();
        let broken = errors.join("broken");
        std::fs::create_dir(&broken).unwrap();
        symlink("subsystem", broken.join("subsystem")).unwrap();
        assert!(
            child_registration(
                super::super::identity::MountIdentity::directory(&root).unwrap(),
                &broken
            )
            .is_err()
        );
        child_devices(&mut document, &errors);
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "Child registration" && fact.error.is_some())
        );
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "Child device inventory"
                    && fact.outcome == Outcome::Unsupported)
        );
        document.facts.clear();
        document.kind = Kind::Kernel {
            filesystem: Filesystem::DeviceNodes,
        };
        child_devices(&mut document, &root);
        assert!(document.facts.is_empty());
    }
    fn fixture(
        name: &str,
        values: &[(&str, &str)],
    ) -> (tempfile::TempDir, PathBuf, Reads, Document) {
        let (temp, root, reads, mut document) = super::super::collector::registered_fixture(
            "fixture",
            &format!("registration-{}", name.replace('/', "-")),
            values,
        );
        let reference = temp.path().join("sysfs").join(name);
        std::fs::create_dir_all(reference.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&root, reference).unwrap();
        document.kind = Kind::Kernel {
            filesystem: Filesystem::Sysfs,
        };
        document.path = root.clone();
        document.interface_directory = true;
        (temp, root, reads, document)
    }
    #[test]
    fn names_alone_do_not_admit_root_schemas_and_large_inventories_are_disclosed() {
        let (temp, root, mut reads, mut document) = fixture("class", &[]);
        for index in 0..160 {
            std::fs::create_dir(root.join(format!("class{index:03}"))).unwrap();
        }
        assert!(!collect_at(
            &mut document,
            &root,
            Path::new("/unrelated"),
            &mut reads,
            &mut |_| {}
        ));
        assert!(collect_at(
            &mut document,
            &root,
            &temp.path().join("sysfs"),
            &mut reads,
            &mut |_| {}
        ));
        assert_eq!(
            document.topology.as_ref().unwrap().coverage,
            Outcome::Truncated
        );
        assert_eq!(document.topology.as_ref().unwrap().nodes.len(), 17);
        assert_eq!(reads.attempts().count(), 0);
    }
    #[test]
    fn bus_counts_are_bounded_registered_names_and_not_device_reads() {
        use std::os::unix::fs::symlink;
        let (temp, root, mut reads, mut document) = fixture("bus", &[]);
        let bus = root.join("usb");
        std::fs::create_dir_all(bus.join("devices")).unwrap();
        std::fs::create_dir(bus.join("drivers")).unwrap();
        symlink("/missing", bus.join("devices/registered")).unwrap();
        std::fs::create_dir(bus.join("drivers/driver")).unwrap();
        collect_at(
            &mut document,
            &root,
            &temp.path().join("sysfs"),
            &mut reads,
            &mut |_| {},
        );
        assert!(document.facts.iter().any(|fact| {
            fact.label == "usb · Registered device names"
                && fact
                    .value
                    .as_deref()
                    .unwrap()
                    .starts_with("1 observed names")
        }));
        assert_eq!(reads.attempts().count(), 0);
    }
    #[test]
    fn sleep_getters_are_scoped_and_do_not_use_handshake_or_transition_controls() {
        let (temp, root, mut reads, mut document) = fixture(
            "power",
            &[
                ("state", "freeze mem disk"),
                ("mem_sleep", "s2idle [deep]"),
                ("disk", "[platform] shutdown"),
                ("wakeup_count", "secret"),
                ("resume", "secret"),
            ],
        );
        collect_at(
            &mut document,
            &root,
            &temp.path().join("sysfs"),
            &mut reads,
            &mut |_| {},
        );
        assert_eq!(reads.attempts().count(), 3);
        assert!(reads.attempts().all(|path| {
            POWER
                .iter()
                .any(|(name, _, _)| path.file_name().unwrap() == *name)
        }));
        document.path = root.join("mem_sleep");
        document.interface_directory = false;
        document.facts.clear();
        collect_at(
            &mut document,
            &root,
            &temp.path().join("sysfs"),
            &mut reads,
            &mut |_| {},
        );
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "Configured suspend mode")
        );
        assert!(
            !document
                .facts
                .iter()
                .any(|fact| fact.label == "Supported system sleep states")
        );
    }
}
