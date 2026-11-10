//! Cached firmware enumeration and registered interfaces; no method invocation.
use super::{
    Document, Relation,
    attributes::{self, Context},
    buses::{self, Topology},
    collector::Reads,
    model::{Fact, Outcome},
};
use std::{
    io,
    path::{Path, PathBuf},
};

fn role(path: &Path) -> Option<&'static str> {
    match buses::subsystem(path).as_deref()? {
        "wmi" => Some("WMI"),
        "acpi" => Some("ACPI"),
        "platform" => Some("Platform"),
        _ => None,
    }
}

pub(super) fn collect(
    document: &mut Document,
    sys: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let root = if role(sys).is_some() {
        sys
    } else if matches!(
        document.kind,
        little_clip_core::kernel_interface::Kind::Character { .. }
    ) && buses::subsystem(sys).as_deref() == Some("misc")
    {
        let Some(root) = sys
            .ancestors()
            .skip(1)
            .take(8)
            .find(|path| role(path) == Some("WMI"))
        else {
            return;
        };
        root
    } else {
        return;
    };
    let Some(kind) = role(root) else {
        let mut fact = Fact::failure(
            "Firmware registration",
            root.into(),
            io::Error::new(
                io::ErrorKind::NotFound,
                "Firmware registration disappeared during collection",
            ),
        );
        fact.outcome = Outcome::Disappeared;
        document.facts.push(fact);
        checkpoint(document);
        return;
    };
    document.title = format!("{kind} firmware/platform device");
    let mut chain = root
        .ancestors()
        .skip(1)
        .take(12)
        .filter(|path| role(path).is_some())
        .map(Path::to_path_buf)
        .collect::<Vec<_>>();
    chain.reverse();
    let mut topology = Topology::new("Firmware/platform relationships", String::new());
    let parent = buses::ancestor_chain(&mut topology, document, chain, None);
    let anchor = buses::add(
        &mut topology,
        document,
        root.into(),
        format!(
            "{kind} · {}",
            root.file_name().unwrap_or_default().to_string_lossy()
        ),
        parent,
    );
    topology.summary = identity(document, root, kind, reads);
    if root != sys {
        buses::add(
            &mut topology,
            document,
            sys.into(),
            "WMI access interface".into(),
            Some(anchor),
        );
    }
    let mut roots = vec![root.to_path_buf()];
    firmware_links(
        document,
        &mut topology,
        root,
        reads,
        anchor,
        &mut roots,
        checkpoint,
    );
    if kind == "WMI" {
        if let Some(platform) = root
            .ancestors()
            .skip(1)
            .take(8)
            .find(|path| role(path) == Some("Platform"))
        {
            remember_root(&mut topology, &mut roots, platform);
            let parent = topology
                .nodes
                .iter()
                .position(|node| node.path == platform)
                .unwrap_or(anchor);
            firmware_links(
                document,
                &mut topology,
                platform,
                reads,
                parent,
                &mut roots,
                checkpoint,
            );
        }
        document.facts.insert(0, Fact::text("WMI semantics", "GUID identifies an interface, not a unique physical device; firmware methods not invoked", root.into()));
    }
    related(document, &mut topology, &roots, root, checkpoint);
    document.topology = Some(topology);
    document.facts.sort_by_key(|fact| fact.interface);
    checkpoint(document);
}

fn identity(document: &mut Document, root: &Path, kind: &str, reads: &mut Reads) -> String {
    let name = root.file_name().unwrap_or_default().to_string_lossy();
    document.facts.push(Fact::text(
        format!("{kind} registered firmware device"),
        format!("{kind} · {name}"),
        root.into(),
    ));
    let context = Context::discover(root);
    for rule in attributes::rules(&context) {
        reads.observe(document, root, &rule);
    }
    if kind == "ACPI" {
        match reads.read(&root.join("modalias")) {
            Ok(alias) => match acpi_ids(&alias) {
                Ok(ids) => {
                    document.facts.push(Fact::text(
                        "ACPI cached enumeration IDs (HID/CID)",
                        ids.join(" · "),
                        root.join("modalias"),
                    ));
                    if let Ok(hid) = reads.read(&root.join("hid")) {
                        let compatible = ids
                            .iter()
                            .filter(|id| **id != hid)
                            .cloned()
                            .collect::<Vec<_>>();
                        let value = if compatible.is_empty() {
                            "No additional IDs in observed alias".into()
                        } else {
                            compatible.join(" · ")
                        };
                        document.facts.push(Fact::text(
                            "ACPI compatible IDs (cached alias)",
                            value,
                            root.join("modalias"),
                        ));
                    }
                }
                Err(error) => document.facts.push(Fact::failure(
                    "ACPI cached enumeration IDs (HID/CID)",
                    root.join("modalias"),
                    error,
                )),
            },
            Err(error) => document.facts.push(Fact::failure(
                "ACPI cached enumeration IDs (HID/CID)",
                root.join("modalias"),
                error,
            )),
        }
        document.facts.push(Fact::text("ACPI observation policy", "Cached enumeration only; cid/status/description/hrv and firmware methods not requested", root.into()));
    }
    let field = if kind == "WMI" {
        "guid"
    } else if kind == "ACPI" {
        "hid"
    } else {
        "modalias"
    };
    let value = reads
        .read(&root.join(field))
        .unwrap_or_else(|_| "identity unavailable".into());
    let driver = buses::driver(root).unwrap_or_else(|| "binding unavailable".into());
    document.facts.push(Fact::text(
        "Registered driver binding",
        driver.clone(),
        root.join("driver"),
    ));
    if let Ok(path) = root.join("driver").canonicalize() {
        document
            .relations
            .push(Relation::new("Firmware/platform driver", path));
    }
    format!("{name} · {value} · {driver}")
}

fn acpi_ids(alias: &str) -> io::Result<Vec<String>> {
    let malformed = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Malformed cached ACPI enumeration alias",
        )
    };
    let line = alias
        .lines()
        .find_map(|line| line.strip_prefix("acpi:"))
        .ok_or_else(malformed)?;
    let mut ids = Vec::new();
    for token in line.split(':').filter(|token| !token.is_empty()) {
        if ids.len() >= 32
            || token.len() > 64
            || !token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(malformed());
        }
        if !ids.iter().any(|id| id == token) {
            ids.push(token.to_string());
        }
    }
    if ids.is_empty() {
        return Err(malformed());
    }
    Ok(ids)
}

fn physical_link(name: &str) -> bool {
    name == "physical_node"
        || name.strip_prefix("physical_node").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn firmware_links(
    document: &mut Document,
    topology: &mut Topology,
    root: &Path,
    reads: &mut Reads,
    anchor: usize,
    roots: &mut Vec<PathBuf>,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            document
                .facts
                .push(Fact::failure("Firmware link inventory", root.into(), error));
            topology.coverage = Outcome::Unsupported;
            return;
        }
    };
    let (paths, coverage) = buses::select_children(
        entries,
        |entry| {
            entry.file_type().is_ok_and(|kind| kind.is_symlink())
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name == "firmware_node" || physical_link(name))
        },
        16,
    );
    if coverage != Outcome::Observed {
        topology.coverage = coverage;
    }
    for path in paths {
        match path.canonicalize() {
            Ok(target) => {
                let label = format!(
                    "{} · {}",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    target.file_name().unwrap_or_default().to_string_lossy()
                );
                if !topology.nodes.iter().any(|node| node.path == target) {
                    buses::add(topology, document, target.clone(), label, Some(anchor));
                }
                document.relations.push(Relation::new(
                    "Observed firmware/physical binding",
                    target.clone(),
                ));
                remember_root(topology, roots, &target);
                if role(&target) == Some("ACPI") {
                    identity(document, &target, "ACPI", reads);
                }
                buses::access_node(document, &target, "Firmware-associated access interface");
            }
            Err(error) => {
                let mut fact = Fact::failure("Firmware relationship", path, error);
                if fact.outcome == Outcome::Missing {
                    fact.outcome = Outcome::Disappeared;
                }
                topology.coverage = fact.outcome.clone();
                document.facts.push(fact);
            }
        }
        checkpoint(document);
    }
}

fn remember_root(topology: &mut Topology, roots: &mut Vec<PathBuf>, root: &Path) {
    if roots.iter().any(|path| path == root) {
        return;
    }
    if roots.len() < 4 {
        roots.push(root.into());
    } else {
        topology.coverage = Outcome::Truncated;
    }
}

fn standard(path: &Path) -> bool {
    matches!(
        buses::subsystem(path).as_deref(),
        Some(
            "hwmon"
                | "leds"
                | "input"
                | "power_supply"
                | "thermal"
                | "rfkill"
                | "platform-profile"
                | "platform_profile"
                | "gpio"
        )
    )
}

fn related(
    document: &mut Document,
    topology: &mut Topology,
    roots: &[PathBuf],
    selected: &Path,
    checkpoint: &mut dyn FnMut(&Document),
) {
    for root in roots {
        let (paths, coverage) = buses::select_descendants(root, 5, 32, standard);
        if coverage != Outcome::Observed {
            topology.coverage = coverage;
        }
        for path in paths {
            if topology.nodes.iter().any(|node| node.path == path) {
                continue;
            }
            let owner = path
                .ancestors()
                .skip(1)
                .take(12)
                .find(|parent| role(parent).is_some());
            // A shared platform ancestor is not evidence that a sibling WMI GUID
            // owns this interface. Preserve the observed owner in the graph.
            if owner
                .is_some_and(|owner| owner != selected && !roots.iter().any(|root| root == owner))
            {
                continue;
            }
            let parent =
                owner.and_then(|owner| topology.nodes.iter().position(|node| node.path == owner));
            let label = format!(
                "{} · {}",
                buses::subsystem(&path).unwrap_or_else(|| "interface unavailable".into()),
                path.file_name().unwrap_or_default().to_string_lossy()
            );
            buses::add(topology, document, path.clone(), label, parent);
            buses::access_node(document, &path, "Exposed standard access interface");
            checkpoint(document);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, os::unix::fs::symlink};

    struct Fixture {
        _directory: tempfile::TempDir,
        platform: PathBuf,
        acpi: PathBuf,
        wmi: PathBuf,
        own: PathBuf,
        sibling: PathBuf,
        shared: PathBuf,
        access: PathBuf,
        values: BTreeMap<PathBuf, Vec<u8>>,
    }
    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let base = directory.path();
            let platform = base.join("devices/platform/system-device");
            let acpi = base.join("devices/acpi/ACME0001:00");
            let wmi = platform.join("wmi_bus/12345678-1234-5678-ABCD-123456789ABC-0");
            let peer = platform.join("wmi_bus/12345678-1234-5678-ABCD-123456789ABC-1");
            let own = wmi.join("hwmon/hwmon0");
            let sibling = peer.join("hwmon/hwmon1");
            let shared = platform.join("input/input0");
            let access = wmi.join("misc/wmi-access");
            for (path, bus) in [
                (&platform, "platform"),
                (&acpi, "acpi"),
                (&wmi, "wmi"),
                (&peer, "wmi"),
                (&own, "hwmon"),
                (&sibling, "hwmon"),
                (&shared, "input"),
                (&access, "misc"),
            ] {
                let subsystem = base.join("bus").join(bus);
                std::fs::create_dir_all(path).unwrap();
                std::fs::create_dir_all(&subsystem).unwrap();
                symlink(subsystem, path.join("subsystem")).unwrap();
            }
            symlink(&acpi, platform.join("firmware_node")).unwrap();
            symlink(&platform, acpi.join("physical_node")).unwrap();
            let driver = base.join("bus/wmi/drivers/example");
            std::fs::create_dir_all(&driver).unwrap();
            symlink(&driver, wmi.join("driver")).unwrap();
            let values = BTreeMap::from([
                (
                    wmi.join("guid"),
                    b"12345678-1234-5678-ABCD-123456789ABC".to_vec(),
                ),
                (wmi.join("instance_count"), b"2".to_vec()),
                (wmi.join("expensive"), b"1".to_vec()),
                (wmi.join("bmof"), b"must not be read".to_vec()),
                (acpi.join("hid"), b"ACME0001".to_vec()),
                (acpi.join("modalias"), b"acpi:ACME0001:PNP0C02:\n".to_vec()),
                (acpi.join("path"), b"\\_SB_.TEST".to_vec()),
                (acpi.join("cid"), b"method invocation forbidden".to_vec()),
                (acpi.join("status"), b"method invocation forbidden".to_vec()),
                (
                    acpi.join("description"),
                    b"method invocation forbidden".to_vec(),
                ),
                (
                    platform.join("modalias"),
                    b"acpi:ACME0001:PNP0C02:".to_vec(),
                ),
            ]);
            Self {
                _directory: directory,
                platform,
                acpi,
                wmi,
                own,
                sibling,
                shared,
                access,
                values,
            }
        }
        fn document(&self) -> Document {
            let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
            document.facts.clear();
            document.relations.clear();
            document.path = self.wmi.clone();
            document
        }
    }

    #[test]
    fn wmi_links_firmware_and_standard_interfaces_without_claiming_sibling_ownership() {
        let fixture = Fixture::new();
        let mut doc = fixture.document();
        let mut reads = Reads::fixture(fixture.values.clone());
        collect(&mut doc, &fixture.wmi, &mut reads, &mut |_| {});
        assert!(doc.title.starts_with("WMI"));
        let topology = doc.topology.as_ref().unwrap();
        assert!(topology.nodes.iter().any(|node| node.path == fixture.own));
        assert!(
            topology
                .nodes
                .iter()
                .any(|node| node.path == fixture.shared)
        );
        assert!(
            !topology
                .nodes
                .iter()
                .any(|node| node.path == fixture.sibling)
        );
        assert!(
            doc.relations
                .iter()
                .any(|relation| relation.target == fixture.acpi)
        );
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "ACPI compatible IDs (cached alias)"
                    && fact.value.as_deref() == Some("PNP0C02"))
        );
        assert!(reads.attempts().all(|path| !matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("bmof" | "cid" | "status" | "description" | "hrv" | "eject" | "driver_override")
        )));
    }

    #[test]
    fn acpi_and_platform_bindings_share_cached_identity_without_firmware_evaluation() {
        let fixture = Fixture::new();
        let mut doc = fixture.document();
        let mut reads = Reads::fixture(fixture.values.clone());
        collect(&mut doc, &fixture.acpi, &mut reads, &mut |_| {});
        assert!(doc.title.starts_with("ACPI"));
        assert!(
            doc.topology
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .any(|node| node.path == fixture.platform)
        );
        assert!(
            doc.topology
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .any(|node| node.path == fixture.shared)
        );
        collect(&mut doc, &fixture.platform, &mut reads, &mut |_| {});
        assert!(doc.title.starts_with("Platform"));
        assert!(
            doc.relations
                .iter()
                .any(|relation| relation.target == fixture.acpi)
        );
        assert!(!reads.attempts().any(|path| path == fixture.acpi.join("cid") || path == fixture.acpi.join("status")));
    }

    #[test]
    fn wmi_access_nodes_use_the_registered_parent_and_inherited_attribute_scopes_are_empty() {
        let fixture = Fixture::new();
        let mut doc = fixture.document();
        doc.kind = little_clip_core::kernel_interface::Kind::Character {
            major: 10,
            minor: 200,
        };
        let mut reads = Reads::fixture(fixture.values.clone());
        collect(&mut doc, &fixture.access, &mut reads, &mut |_| {});
        assert!(
            doc.topology
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .any(|node| node.path == fixture.access)
        );
        let nested = fixture.wmi.join("private-data");
        std::fs::create_dir(&nested).unwrap();
        let context = Context::discover(&nested);
        assert!(!context.direct);
        assert!(attributes::rules(&context).is_empty());
        let mut doc = fixture.document();
        doc.kind = little_clip_core::kernel_interface::Kind::Character {
            major: 10,
            minor: 200,
        };
        collect(&mut doc, &fixture.own, &mut reads, &mut |_| {});
        assert!(
            doc.topology.is_none(),
            "Standard child interfaces keep their specialized collector"
        );
    }

    #[test]
    fn cached_aliases_and_physical_link_names_are_bounded_and_validated() {
        assert_eq!(
            acpi_ids("acpi:PNP0C14:PNP0C14:ACPI0001:\nof:test").unwrap(),
            ["PNP0C14", "ACPI0001"]
        );
        for alias in ["", "of:example", "acpi:", "acpi:PNP0C14:*:"] {
            assert!(acpi_ids(alias).is_err());
        }
        assert!(acpi_ids(&format!("acpi:{}:", "X".repeat(65))).is_err());
        assert!(
            acpi_ids(&format!(
                "acpi:{}",
                (0..33).map(|id| format!("ID{id}:")).collect::<String>()
            ))
            .is_err()
        );
        for name in ["physical_node", "physical_node1", "physical_node99"] {
            assert!(physical_link(name));
        }
        for name in ["physical_node_backup", "physical_node-1", "firmware_method"] {
            assert!(!physical_link(name));
        }
        let context =
            Context::from_parts(Path::new("/sys/bus/acpi/devices/PNP0C14:00"), Some("acpi"));
        for field in [
            "cid",
            "status",
            "description",
            "hrv",
            "eject",
            "real_power_state",
        ] {
            assert!(attributes::rule(&context, field).is_none());
        }
        let power_device =
            Context::from_parts(Path::new("/sys/devices/platform/power"), Some("platform"));
        assert!(attributes::rule(&power_device, "modalias").is_some());
        assert!(attributes::rule(&power_device, "control").is_none());
    }

    #[test]
    fn firmware_relationship_removal_and_root_limits_are_disclosed() {
        let fixture = Fixture::new();
        std::fs::remove_file(fixture.platform.join("firmware_node")).unwrap();
        symlink(
            fixture.platform.join("removed"),
            fixture.platform.join("firmware_node"),
        )
        .unwrap();
        let mut doc = fixture.document();
        let mut reads = Reads::fixture(fixture.values.clone());
        collect(&mut doc, &fixture.platform, &mut reads, &mut |_| {});
        assert_eq!(
            doc.topology.as_ref().unwrap().coverage,
            Outcome::Disappeared
        );
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "Firmware relationship"
                    && fact.outcome == Outcome::Disappeared)
        );
        let mut roots = vec![
            fixture.platform.clone(),
            fixture.acpi.clone(),
            fixture.wmi.clone(),
            fixture.own.clone(),
        ];
        let topology = doc.topology.as_mut().unwrap();
        remember_root(topology, &mut roots, &fixture.shared);
        assert_eq!(topology.coverage, Outcome::Truncated);
        assert_eq!(roots.len(), 4);
    }
}
