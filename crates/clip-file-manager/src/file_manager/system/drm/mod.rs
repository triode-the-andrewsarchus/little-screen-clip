//! Display-controller/connector metadata only: no DRM opens, KMS or framebuffer IO.
mod edid;
use super::{
    Document, Relation,
    buses::{self, Topology},
    collector::Reads,
    model::{Fact, Outcome},
};
use std::{
    io,
    path::{Path, PathBuf},
};

fn access_role(name: &str) -> &'static str {
    if name
        .split_once('-')
        .is_some_and(|(prefix, _)| prefix.starts_with("card"))
    {
        "DRM connector"
    } else if name.starts_with("renderD") {
        "DRM render access node"
    } else if name.starts_with("controlD") {
        "DRM legacy control node"
    } else {
        "DRM primary access node"
    }
}

pub(super) fn directory_group(sys: &Path) -> Option<(Option<PathBuf>, String)> {
    if buses::subsystem(sys).as_deref() != Some("drm") {
        return None;
    }
    let name = sys.file_name()?.to_str()?;
    if !["card", "renderD", "controlD"]
        .iter()
        .any(|prefix| buses::numbered(name, prefix))
    {
        return None;
    }
    Some((
        sys.join("device").canonicalize().ok(),
        access_role(name).into(),
    ))
}

pub(super) fn connected_outputs(topology: &Topology) -> usize {
    topology
        .nodes
        .iter()
        .filter(|node| node.label.contains(" · connected · "))
        .count()
}

pub(super) fn collect(document: &mut Document, sys: &Path, reads: &mut Reads) {
    if buses::subsystem(sys).as_deref() != Some("drm") {
        return;
    }
    let name = sys.file_name().unwrap_or_default().to_string_lossy();
    let connector = name
        .split_once('-')
        .filter(|(prefix, _)| prefix.starts_with("card"));
    let card = if let Some((prefix, _)) = connector {
        PathBuf::from("/sys/class/drm")
            .join(prefix)
            .canonicalize()
            .ok()
    } else if name.starts_with("card") {
        Some(sys.into())
    } else {
        None
    };
    let gpu = card
        .as_ref()
        .unwrap_or(&sys.to_path_buf())
        .join("device")
        .canonicalize()
        .ok();
    let role = access_role(&name);
    document.title = role.into();
    let mut topology = Topology::new("Display connectors", format!("{name} · {role}"));
    let mut root = None;
    if let Some(gpu) = &gpu {
        let driver = buses::driver(gpu).unwrap_or_else(|| "binding unavailable".into());
        topology.summary.push_str(&format!(" · {driver}"));
        root = Some(buses::add(
            &mut topology,
            document,
            gpu.clone(),
            format!(
                "{} · {driver}",
                gpu.file_name().unwrap_or_default().to_string_lossy()
            ),
            None,
        ));
    }
    if let Some(card) = card {
        let parent = buses::add(
            &mut topology,
            document,
            card.clone(),
            card.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
            root,
        );
        connectors(
            document,
            &card,
            reads,
            &mut topology,
            parent,
            connector.map(|_| sys),
            true,
        );
        if connector.is_some() {
            document
                .relations
                .push(Relation::new("DRM primary node", card));
        }
    } else if let Some(gpu) = &gpu {
        primary_cards(document, gpu, reads, &mut topology, root, true);
    }
    if connector.is_some() {
        if let Some(selected) = topology.nodes.iter().find(|node| node.path == sys) {
            topology.summary = selected.label.clone();
        }
    } else {
        let connected = connected_outputs(&topology);
        topology
            .summary
            .push_str(&format!(" · {connected} observed connected outputs"));
    }
    if let Some(gpu) = &gpu {
        related_nodes(document, gpu, &mut topology);
    }
    document.facts.push(Fact::text("Display interpretation","Connected is presence detection; enabled reports an encoder association. Mode names and EDID timings are advertised, not a queried active mode. No DRM master or framebuffer access.",sys.into()));
    document.topology = Some(topology);
}
pub(super) fn primary_cards(
    document: &mut Document,
    gpu: &Path,
    reads: &mut Reads,
    topology: &mut Topology,
    root: Option<usize>,
    detailed: bool,
) {
    match std::fs::read_dir(gpu.join("drm")) {
        Ok(entries) => {
            let (paths, coverage) = buses::select_children(
                entries,
                |entry| buses::numbered(&entry.file_name().to_string_lossy(), "card"),
                32,
            );
            if coverage != Outcome::Observed {
                topology.coverage = coverage;
            }
            for path in paths {
                let parent = buses::add(
                    topology,
                    document,
                    path.clone(),
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into(),
                    root,
                );
                connectors(document, &path, reads, topology, parent, None, detailed);
            }
        }
        Err(error) => {
            let fact = Fact::failure("DRM primary-node inventory", gpu.join("drm"), error);
            topology.coverage = fact.outcome.clone();
            document.facts.push(fact);
        }
    }
}
fn related_nodes(document: &mut Document, gpu: &Path, topology: &mut Topology) {
    let path = gpu.join("drm");
    let entries = match std::fs::read_dir(&path) {
        Ok(entries) => entries,
        Err(error) => {
            topology.inventory_failure(document, "DRM sibling inventory", &path, error);
            topology
                .summary
                .push_str(" · sibling inventory unavailable");
            return;
        }
    };
    let (paths, coverage) = buses::select_children(
        entries,
        |entry| buses::subsystem(&entry.path()).as_deref() == Some("drm"),
        32,
    );
    if coverage != Outcome::Observed
        && (topology.coverage == Outcome::Observed || coverage == Outcome::Truncated)
    {
        topology.coverage = coverage.clone();
    }
    if coverage != Outcome::Observed {
        topology
            .summary
            .push_str(&format!(" · sibling inventory: {coverage}"));
    }
    let mut fact = Fact::text(
        "DRM sibling inventory",
        format!(
            "{} observed sibling interfaces retained; at most 128 entries scanned and 32 retained. This is registration metadata, not active device use.",
            paths.len()
        ),
        path,
    );
    fact.outcome = coverage;
    document.facts.push(fact);
    for path in paths {
        document
            .relations
            .push(Relation::new("Related DRM interface", path.clone()));
        buses::access_node(document, &path, "DRM access node");
    }
}

fn connectors(
    document: &mut Document,
    card: &Path,
    reads: &mut Reads,
    topology: &mut Topology,
    parent: usize,
    selected: Option<&Path>,
    detailed: bool,
) {
    // Explicit inspection must not depend on the card overview's inventory cap.
    let paths = if let Some(selected) = selected {
        vec![selected.to_path_buf()]
    } else {
        let entries = match std::fs::read_dir(card) {
            Ok(entries) => entries,
            Err(error) => {
                topology.coverage = Outcome::Unsupported;
                document
                    .facts
                    .push(Fact::failure("Connector inventory", card.into(), error));
                return;
            }
        };
        let prefix = format!(
            "{}-",
            card.file_name().unwrap_or_default().to_string_lossy()
        );
        let (paths, coverage) = buses::select_children(
            entries,
            |entry| {
                entry.file_name().to_string_lossy().starts_with(&prefix)
                    && buses::subsystem(&entry.path()).as_deref() == Some("drm")
            },
            8,
        );
        if coverage != Outcome::Observed {
            topology.coverage = coverage;
        }
        paths
    };
    for path in paths {
        let status = reads
            .read(&path.join("status"))
            .unwrap_or_else(|_| "unavailable".into());
        let enabled = if detailed {
            reads
                .read(&path.join("enabled"))
                .unwrap_or_else(|_| "unavailable".into())
        } else {
            "encoder state not queried".into()
        };
        let fields: &[(&str, &str)] = if detailed {
            &[
                ("status", "Connection status"),
                ("enabled", "Encoder association"),
                ("modes", "Advertised mode names"),
            ]
        } else {
            &[("status", "Connection status")]
        };
        for &(attribute, label) in fields {
            let source = path.join(attribute);
            let fact = match reads.read(&source) {
                Ok(value) => Fact::text(
                    format!(
                        "{} · {label}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ),
                    value,
                    source,
                ),
                Err(error) => Fact::failure(label, source, error),
            };
            if fact.outcome != Outcome::Observed && topology.coverage == Outcome::Observed {
                topology.coverage = fact.outcome.clone();
            }
            document.facts.push(fact);
        }
        let monitor = detailed
            .then(|| edid_facts(document, &path, reads))
            .flatten();
        let label = format!(
            "{} · {status} · {enabled}{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            monitor
                .and_then(|monitor| monitor.name)
                .map(|name| format!(" · {name}"))
                .unwrap_or_default()
        );
        buses::add(topology, document, path, label, Some(parent));
    }
}
pub(super) fn edid_facts(
    document: &mut Document,
    connector: &Path,
    reads: &mut Reads,
) -> Option<edid::Monitor> {
    let source = connector.join("edid");
    let result = reads.edid(&source).and_then(|bytes| {
        if bytes.is_empty() {
            return Err(io::Error::from(io::ErrorKind::NotFound));
        }
        edid::parse(&bytes)
    });
    let monitor = match result {
        Ok(monitor) => monitor,
        Err(error) => {
            document
                .facts
                .push(Fact::failure("EDID identity", source, error));
            return None;
        }
    };
    let mut values = vec![
        ("EDID manufacturer", monitor.manufacturer.clone()),
        ("EDID product", format!("{:04x}", monitor.product)),
        ("EDID manufacture/model year", monitor.year.to_string()),
    ];
    if monitor.serial != 0 {
        values.push(("EDID numeric serial", monitor.serial.to_string()));
    }
    if let Some(name) = &monitor.name {
        values.push(("Monitor name", name.clone()));
    }
    if let Some(serial) = &monitor.text_serial {
        values.push(("Monitor serial", serial.clone()));
    }
    if let Some((width, height)) = monitor.size {
        values.push(("EDID physical size", format!("{width}×{height} cm")));
    }
    for timing in &monitor.timings {
        values.push(("Advertised base EDID detailed timing", timing.clone()));
    }
    values.push(("EDID coverage",format!("{} validated extension blocks; extension timings not decoded here. Sysfs advertised mode names are shown separately.",monitor.extensions)));
    for (label, value) in values {
        let mut fact = Fact::text(label, value, source.clone());
        fact.origin = "validated EDID metadata".into();
        document.facts.push(fact);
    }
    Some(monitor)
}

#[cfg(test)]
mod tests {
    use super::super::collector::registered_fixture;
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn selected_card_discloses_bounded_sibling_inventory() {
        let (temp, card, mut reads, mut document) =
            super::super::collector::registered_fixture("drm", "card0", &[]);
        let gpu = temp.path().join("gpu");
        std::fs::create_dir_all(gpu.join("drm")).unwrap();
        symlink(&gpu, card.join("device")).unwrap();
        symlink(&card, gpu.join("drm/card0")).unwrap();
        let subsystem = card.join("subsystem").canonicalize().unwrap();
        for index in 0..40 {
            let node = gpu.join("drm").join(format!("renderD{}", 128 + index));
            std::fs::create_dir(&node).unwrap();
            symlink(&subsystem, node.join("subsystem")).unwrap();
        }
        collect(&mut document, &card, &mut reads);
        let fact = document
            .facts
            .iter()
            .find(|fact| fact.label == "DRM sibling inventory")
            .expect("bounded sibling inventory must disclose its coverage");
        assert!(document.summary().contains("sibling inventory: Truncated"));
        assert_eq!(fact.outcome, Outcome::Truncated);
        assert_eq!(
            document.topology.as_ref().unwrap().coverage,
            Outcome::Truncated
        );
        assert_eq!(
            document
                .relations
                .iter()
                .filter(|relation| relation.label == "Related DRM interface")
                .count(),
            32
        );
        assert_eq!(reads.attempts().count(), 0);
    }

    #[test]
    fn selected_card_distinguishes_unavailable_siblings_from_an_empty_inventory() {
        for state in ["missing", "not_directory", "empty"] {
            let (temp, card, mut reads, mut document) =
                super::super::collector::registered_fixture("drm", "card0", &[]);
            let gpu = temp.path().join("gpu");
            std::fs::create_dir(&gpu).unwrap();
            symlink(&gpu, card.join("device")).unwrap();
            let expected = match state {
                "not_directory" => {
                    std::fs::write(gpu.join("drm"), "metadata only fixture").unwrap();
                    Outcome::Unsupported
                }
                "empty" => {
                    std::fs::create_dir(gpu.join("drm")).unwrap();
                    Outcome::Observed
                }
                _ => Outcome::Missing,
            };
            collect(&mut document, &card, &mut reads);
            let fact = document
                .facts
                .iter()
                .find(|fact| fact.label == "DRM sibling inventory")
                .expect("sibling inventory must record errors as well as success");
            assert_eq!(fact.outcome, expected, "{state}");
            assert_eq!(
                document.topology.as_ref().unwrap().coverage,
                expected,
                "{state}"
            );
            if state == "empty" {
                assert!(fact.value.as_deref().unwrap().starts_with("0 "));
            } else {
                assert!(document.summary().contains("sibling inventory unavailable"));
                assert!(fact.error.is_some());
                assert!(fact.value.is_none());
            }
            assert_eq!(reads.attempts().count(), 0);
        }
    }

    #[test]
    fn explicit_connector_is_observed_without_reading_parent_inventory() {
        let (temp, connector, mut reads, mut document) = registered_fixture(
            "drm",
            "card0-DP-9",
            &[
                ("status", "connected"),
                ("enabled", "enabled"),
                ("modes", "1920x1080"),
            ],
        );
        // A missing parent inventory must not suppress a selected connector.
        let card = temp.path().join("card0");
        document.facts.clear();
        let mut topology = Topology::new("Display connectors", String::new());
        connectors(
            &mut document,
            &card,
            &mut reads,
            &mut topology,
            0,
            Some(&connector),
            true,
        );
        assert_eq!(topology.nodes.len(), 1);
        assert_eq!(topology.nodes[0].path, connector);
        assert!(topology.nodes[0].label.contains("connected · enabled"));
        assert_eq!(topology.coverage, Outcome::Observed);
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label.ends_with("Advertised mode names")
                    && fact.value.as_deref() == Some("1920x1080"))
        );
        assert!(
            !document
                .facts
                .iter()
                .any(|fact| fact.label == "Connector inventory")
        );
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "EDID identity" && fact.outcome == Outcome::Missing)
        );
    }
}
