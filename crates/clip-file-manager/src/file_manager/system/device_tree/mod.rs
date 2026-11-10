//! Bounded device-tree property decoding and observed kernel relationships.
mod parse;
use super::{
    Document, Relation,
    buses::{self, Topology},
    collector::Reads,
    model::{Fact, Outcome, Value},
};
use std::{
    collections::BTreeMap,
    io,
    path::{Component, Path, PathBuf},
};

const FIELDS: &[&str] = &[
    "compatible",
    "model",
    "status",
    "reg",
    "interrupts",
    "interrupts-extended",
    "interrupt-parent",
    "#address-cells",
    "#size-cells",
    "#interrupt-cells",
    "phandle",
    "linux,phandle",
    "interrupt-controller",
];

pub(super) fn approved(path: &Path, root: &Path) -> bool {
    path.strip_prefix(root).is_ok_and(|relative| {
        relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    }) && path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| FIELDS.contains(&name))
}

struct Properties<'a> {
    root: &'a Path,
    reads: &'a mut Reads,
    cache: BTreeMap<PathBuf, Result<Vec<u8>, (io::ErrorKind, String)>>,
}
impl Properties<'_> {
    fn get(&mut self, node: &Path, name: &str) -> io::Result<Vec<u8>> {
        let path = node.join(name);
        if !self.cache.contains_key(&path) {
            let value = self
                .reads
                .device_tree(&path, self.root)
                .map_err(|error| (error.kind(), error.to_string()));
            self.cache.insert(path.clone(), value);
        }
        self.cache[&path]
            .clone()
            .map_err(|(kind, message)| io::Error::new(kind, message))
    }
    fn width(&mut self, parent: &Path, name: &str, default: u32) -> io::Result<u32> {
        match self.get(parent, name) {
            Ok(bytes) => parse::scalar(&bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(default),
            Err(error) => Err(error),
        }
    }
}

pub(super) fn inspect(
    document: &mut Document,
    sys: &Path,
    requested: Option<&str>,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) -> bool {
    let authority = Path::new("/sys/firmware/devicetree/base");
    if !authority.is_dir() {
        return false;
    }
    let Some(root) = sys
        .ancestors()
        .take(64)
        .find(|ancestor| super::boot_firmware::same_object(ancestor, authority))
    else {
        return false;
    };
    collect(document, sys, root, requested, reads, checkpoint);
    true
}

fn collect(
    document: &mut Document,
    node: &Path,
    root: &Path,
    requested: Option<&str>,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    document.title = if requested.is_some() {
        "Device-tree property"
    } else {
        "Device-tree node"
    }
    .into();
    let mut properties = Properties {
        root,
        reads,
        cache: BTreeMap::new(),
    };
    if let Some(name) = requested {
        if FIELDS.contains(&name) {
            field(document, node, name, &mut properties);
        } else {
            let mut fact = Fact::text(
                "Device-tree property policy",
                "Unknown property; bytes not read",
                node.join(name),
            );
            fact.outcome = Outcome::NotRequested;
            document.facts.insert(0, fact);
        }
        checkpoint(document);
        return;
    }
    let name = if node == root {
        "/".into()
    } else {
        node.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    };
    document.facts.insert(
        0,
        Fact::text("Device-tree node name", name.clone(), node.into()),
    );
    for field_name in FIELDS {
        field(document, node, field_name, &mut properties);
        checkpoint(document);
    }
    let mut topology = Topology::new("Device-tree hierarchy", name);
    for (field, label) in [("compatible", "compatible"), ("status", "status")] {
        if let Some(value) = document
            .facts
            .iter()
            .find(|fact| {
                fact.source == node.join(field)
                    && fact.outcome == Outcome::Observed
                    && !fact.retained
            })
            .and_then(|fact| fact.value.as_ref())
        {
            let summary = value.chars().take(80).collect::<String>();
            topology.summary.push_str(&format!(" · {label} {summary}"));
        }
    }
    let mut chain = node
        .ancestors()
        .take_while(|path| path.starts_with(root))
        .take(64)
        .map(Path::to_path_buf)
        .collect::<Vec<_>>();
    chain.reverse();
    let anchor = buses::ancestor_chain(&mut topology, document, chain, None).unwrap_or(0);
    let (children, coverage) = buses::select_descendants(node, 2, 16, |path| path != node);
    topology.coverage = coverage;
    for path in children {
        let parent = path
            .parent()
            .and_then(|parent| topology.nodes.iter().position(|node| node.path == parent));
        let label = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        buses::add(
            &mut topology,
            document,
            path,
            label,
            parent.or(Some(anchor)),
        );
    }
    document.facts.push(Fact::text(
        "DT hierarchy scope",
        "Two descendant levels; at most 16 preview nodes; not a full-tree inventory",
        node.into(),
    ));
    document.topology = Some(topology.clone());
    checkpoint(document);
    interrupt_parent(
        document,
        node,
        &mut topology,
        &mut properties,
        anchor,
        checkpoint,
    );
    platforms(
        document,
        node,
        &mut topology,
        anchor,
        Path::new("/sys/bus/platform/devices"),
    );
    document.facts.push(Fact::text("Device-tree interpretation", "Bus-relative resources and raw interrupt specifiers; no register/memory access or hardware-health inference", node.into()));
    document.topology = Some(topology);
    document.facts.sort_by_key(|fact| fact.interface);
    checkpoint(document);
}

fn field(document: &mut Document, node: &Path, name: &str, properties: &mut Properties<'_>) {
    let raw = properties.get(node, name);
    let result = raw
        .as_ref()
        .map_err(|error| io::Error::new(error.kind(), error.to_string()))
        .and_then(|bytes| decode(bytes, node, name, properties))
        .and_then(|value| {
            if value.chars().count() <= 4096 {
                Ok(value)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Device-tree display exceeds 4096 characters",
                ))
            }
        });
    let mut fact = match result {
        Ok(value) => Fact::text(format!("DT {name}"), value, node.join(name)),
        Err(error) => Fact::failure(format!("DT {name}"), node.join(name), error),
    };
    fact.origin = "documented device-tree binary property".into();
    if let Ok(bytes) = raw {
        if fact.outcome == Outcome::Observed {
            match name {
                "#address-cells" | "#size-cells" | "#interrupt-cells" | "phandle"
                | "linux,phandle" | "interrupt-parent" => {
                    if let Ok(value) = parse::scalar(&bytes) {
                        fact.semantic = Some(Value::Integer(value.into()));
                    }
                }
                "interrupt-controller" => fact.semantic = Some(Value::Boolean(true)),
                _ => {}
            }
        }
        let mut raw = bytes
            .iter()
            .take(1024)
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(" ");
        if bytes.len() > 1024 {
            raw.push_str(&format!(
                " … ({} bytes; raw snapshot truncated)",
                bytes.len()
            ));
        }
        fact.raw = Some(raw);
    }
    document.facts.push(fact);
}

fn decode(
    bytes: &[u8],
    node: &Path,
    name: &str,
    properties: &mut Properties<'_>,
) -> io::Result<String> {
    match name {
        "compatible" | "model" | "status" => {
            let strings = parse::strings(bytes)?;
            if strings.iter().any(String::is_empty) || (name != "compatible" && strings.len() != 1)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid device-tree string cardinality",
                ));
            }
            Ok(strings.join(" · "))
        }
        "reg" => {
            let parent = node
                .parent()
                .filter(|parent| parent.starts_with(properties.root))
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::Unsupported,
                        "Root register schema has no observed parent",
                    )
                })?;
            let address = properties.width(parent, "#address-cells", 2)?;
            let size = properties.width(parent, "#size-cells", 1)?;
            Ok(format!(
                "{}; parent schema {address} address cells/{size} size cells (DT defaults 2/1 when absent)",
                parse::registers(bytes, address, size)?.join("; ")
            ))
        }
        "interrupt-controller" if bytes.is_empty() => Ok("Boolean property present".into()),
        "interrupt-controller" => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Device-tree boolean property is not empty",
        )),
        "interrupts" | "interrupts-extended" => {
            let cells = parse::cells(bytes)?;
            if cells.len() > 32 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Interrupt display exceeds 32 specifier cells",
                ));
            }
            Ok(format!(
                "Binding-specific specifier cells: {}",
                cells
                    .iter()
                    .map(|cell| format!("0x{cell:x}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ))
        }
        _ => Ok(format!("{} (big-endian u32)", parse::scalar(bytes)?)),
    }
}

fn interrupt_parent(
    document: &mut Document,
    node: &Path,
    topology: &mut Topology,
    properties: &mut Properties<'_>,
    anchor: usize,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let root = properties.root;
    let explicit = node
        .ancestors()
        .take_while(|path| path.starts_with(root))
        .take(64)
        .find_map(|path| match properties.get(path, "interrupt-parent") {
            Ok(bytes) => Some((path.join("interrupt-parent"), parse::scalar(&bytes))),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => Some((path.join("interrupt-parent"), Err(error))),
        });
    let Some((source, value)) = explicit else {
        if node != root && properties.get(node, "interrupts").is_ok() {
            if let Some(parent) = node.parent() {
                document.relations.push(Relation::new(
                    "Default interrupt-tree parent (binding unverified)",
                    parent.into(),
                ));
            }
        }
        return;
    };
    let value = match value {
        Ok(value) if value != 0 && value != u32::MAX => value,
        Ok(_) => {
            document.facts.push(Fact::failure(
                "DT interrupt parent",
                source,
                io::Error::new(io::ErrorKind::InvalidData, "Reserved phandle value"),
            ));
            return;
        }
        Err(error) => {
            document
                .facts
                .push(Fact::failure("DT interrupt parent", source, error));
            return;
        }
    };
    let (found, coverage) = resolve_phandle(properties, value);
    if coverage != Outcome::Observed {
        topology.coverage = coverage.clone();
    }
    if found.len() == 1 {
        let target = found[0].clone();
        let label = format!("Observed interrupt-parent phandle 0x{value:x}");
        if !topology.nodes.iter().any(|node| node.path == target) {
            buses::add(
                topology,
                document,
                target.clone(),
                label.clone(),
                Some(anchor),
            );
        }
        document.relations.push(Relation::new(label, target));
    }
    let mut fact = Fact::text(
        "DT interrupt-parent lookup",
        format!(
            "Phandle 0x{value:x}; {} observed matches; lookup coverage {coverage}",
            found.len()
        ),
        source,
    );
    fact.outcome = if found.len() > 1 {
        Outcome::Malformed
    } else if found.is_empty() && coverage == Outcome::Observed {
        Outcome::Missing
    } else {
        coverage
    };
    document.facts.push(fact);
    checkpoint(document);
}

fn resolve_phandle(properties: &mut Properties<'_>, value: u32) -> (Vec<PathBuf>, Outcome) {
    let (nodes, mut coverage) = buses::walk(properties.root, 32, 128);
    if nodes.iter().any(|node| {
        node.strip_prefix(properties.root)
            .is_ok_and(|path| path.components().count() >= 32)
    }) {
        coverage = Outcome::Truncated;
    }
    let mut found = Vec::new();
    for path in nodes {
        let value_read = match properties.get(&path, "phandle") {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                properties.get(&path, "linux,phandle")
            }
            result => result,
        };
        match value_read.and_then(|bytes| parse::scalar(&bytes)) {
            Ok(handle) if handle == value => found.push(path),
            Err(error) if error.kind() != io::ErrorKind::NotFound => coverage = Outcome::Truncated,
            _ => {}
        }
    }
    (found, coverage)
}

fn platforms(
    document: &mut Document,
    node: &Path,
    topology: &mut Topology,
    anchor: usize,
    directory: &Path,
) {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            document.facts.push(Fact::failure(
                "DT platform inventory",
                directory.into(),
                error,
            ));
            return;
        }
    };
    let (paths, coverage) = buses::select_children(
        entries,
        |entry| {
            entry
                .path()
                .join("of_node")
                .canonicalize()
                .is_ok_and(|path| super::boot_firmware::same_object(&path, node))
        },
        16,
    );
    for path in paths {
        let path = path.canonicalize().unwrap_or(path);
        buses::add(
            topology,
            document,
            path.clone(),
            format!(
                "Associated platform device {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            ),
            Some(anchor),
        );
        buses::access_node(document, &path, "Device-tree platform access interface");
    }
    if coverage != Outcome::Observed {
        topology.coverage = coverage;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn encoded(cells: &[u32]) -> Vec<u8> {
        cells.iter().flat_map(|cell| cell.to_be_bytes()).collect()
    }

    #[test]
    fn node_collection_decodes_parent_schema_and_interrupt_targets_without_payload_probing() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("base");
        let bus = root.join("soc");
        let node = bus.join("uart@1000");
        let controller = root.join("interrupt-controller");
        for path in [&node, &controller] {
            std::fs::create_dir_all(path).unwrap();
        }
        let values = BTreeMap::from([
            (bus.join("#address-cells"), encoded(&[2])),
            (bus.join("#size-cells"), encoded(&[1])),
            (node.join("compatible"), b"vendor,uart\0ns16550\0".to_vec()),
            (node.join("status"), b"okay\0".to_vec()),
            (node.join("reg"), encoded(&[1, 0x1000, 0x100])),
            (node.join("interrupt-parent"), encoded(&[7])),
            (node.join("interrupts"), encoded(&[10, 8])),
            (controller.join("phandle"), encoded(&[7])),
            (root.join("bootargs"), b"private boot args".to_vec()),
        ]);
        let mut reads = Reads::fixture(values);
        let mut doc = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        doc.facts.clear();
        doc.relations.clear();
        collect(&mut doc, &node, &root, None, &mut reads, &mut |_| {});
        assert!(doc.facts.iter().any(|fact| fact.label == "DT compatible"
            && fact.value.as_deref() == Some("vendor,uart · ns16550")));
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "DT interrupt-parent"
                    && fact.semantic == Some(Value::Integer(7))
                    && fact.raw.as_deref() == Some("00 00 00 07"))
        );
        assert!(doc.facts.iter().any(|fact| {
            fact.label == "DT reg"
                && fact
                    .value
                    .as_ref()
                    .is_some_and(|value| value.contains("0x100001000"))
        }));
        assert!(
            doc.relations
                .iter()
                .any(|relation| relation.target == controller
                    && relation.label.starts_with("Observed interrupt-parent"))
        );
        assert!(doc.facts.iter().any(|fact| {
            fact.label == "DT interrupt-parent lookup"
                && fact
                    .value
                    .as_ref()
                    .is_some_and(|value| value.contains("1 observed matches"))
        }));
        assert!(approved(&node.join("compatible"), &root));
        assert!(!approved(&root.join("../compatible"), &root));
        assert!(!approved(&root.join("bootargs"), &root));
        assert!(!approved(&root.join("serial-number"), &root));
        assert!(reads.device_tree(&root.join("bootargs"), &root).is_err());
        let platforms_dir = temp.path().join("platforms");
        let device = platforms_dir.join("example");
        std::fs::create_dir_all(&device).unwrap();
        symlink(&node, device.join("of_node")).unwrap();
        let mut topology = doc.topology.take().unwrap();
        platforms(&mut doc, &node, &mut topology, 0, &platforms_dir);
        assert!(topology.nodes.iter().any(|node| node.path == device));
    }

    #[test]
    fn missing_parent_cell_counts_use_defaults_not_grandparent_values() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("base");
        let parent = root.join("parent");
        let node = parent.join("device");
        std::fs::create_dir_all(&node).unwrap();
        let mut reads = Reads::fixture(BTreeMap::from([
            (root.join("#address-cells"), encoded(&[1])),
            (root.join("#size-cells"), encoded(&[0])),
            (node.join("reg"), encoded(&[0, 0x1234, 0x10])),
        ]));
        let mut props = Properties {
            root: &root,
            reads: &mut reads,
            cache: BTreeMap::new(),
        };
        let bytes = props.get(&node, "reg").unwrap();
        let value = decode(&bytes, &node, "reg", &mut props).unwrap();
        assert!(value.contains("2 address cells/1 size cells"));
        assert!(value.contains("0x1234"));
        assert!(!props.cache.contains_key(&root.join("#address-cells")));
    }

    #[test]
    fn duplicate_phandles_do_not_create_an_ambiguous_relationship() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("base");
        let node = root.join("device");
        let first = root.join("a");
        let second = root.join("b");
        for path in [&node, &first, &second] {
            std::fs::create_dir_all(path).unwrap();
        }
        let mut reads = Reads::fixture(BTreeMap::from([
            (node.join("interrupt-parent"), encoded(&[9])),
            (first.join("phandle"), encoded(&[9])),
            (second.join("linux,phandle"), encoded(&[9])),
        ]));
        let mut doc = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        doc.relations.clear();
        let mut props = Properties {
            root: &root,
            reads: &mut reads,
            cache: BTreeMap::new(),
        };
        let mut topology = Topology::new("test", String::new());
        interrupt_parent(&mut doc, &node, &mut topology, &mut props, 0, &mut |_| {});
        assert!(
            !doc.relations
                .iter()
                .any(|relation| relation.label.starts_with("Observed interrupt-parent"))
        );
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "DT interrupt-parent lookup"
                    && fact.outcome == Outcome::Malformed)
        );
    }

    #[test]
    fn malformed_properties_unknown_requests_and_large_lookups_are_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("base");
        std::fs::create_dir(&root).unwrap();
        let mut values = BTreeMap::from([(root.join("status"), b"not terminated".to_vec())]);
        for id in 0..140 {
            let node = root.join(format!("node{id}"));
            std::fs::create_dir(&node).unwrap();
            values.insert(node.join("phandle"), encoded(&[id + 1]));
        }
        let mut reads = Reads::fixture(values);
        let mut doc = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        collect(
            &mut doc,
            &root,
            &root,
            Some("status"),
            &mut reads,
            &mut |_| {},
        );
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "DT status" && fact.outcome == Outcome::Malformed)
        );
        collect(
            &mut doc,
            &root,
            &root,
            Some("bootargs"),
            &mut reads,
            &mut |_| {},
        );
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "Device-tree property policy"
                    && fact.outcome == Outcome::NotRequested)
        );
        let mut props = Properties {
            root: &root,
            reads: &mut reads,
            cache: BTreeMap::new(),
        };
        let (_, coverage) = resolve_phandle(&mut props, 999);
        assert_eq!(coverage, Outcome::Truncated);
        let mut props = Properties {
            root: &root,
            reads: &mut reads,
            cache: BTreeMap::new(),
        };
        assert!(
            props.get(&root.join("node0"), "phandle").is_err(),
            "Shared successful-read budget remains exhausted"
        );
    }

    #[test]
    fn device_tree_reader_rejects_ordinary_files_and_oversized_payloads() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("compatible");
        std::fs::write(&path, b"ordinary data\0").unwrap();
        let mut reads = Reads::new();
        assert_eq!(
            reads.device_tree(&path, temp.path()).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        let mut reads = Reads::fixture(BTreeMap::from([(path.clone(), vec![0; 16 * 1024])]));
        assert_eq!(
            reads.device_tree(&path, temp.path()).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn hierarchy_strip_remains_inside_its_row_in_dark_and_light() {
        use little_clip_core::{config::WorkbenchAppearance, palette::AppPalette};
        let mut topology = Topology::new("Device-tree hierarchy", "soc · uart".into());
        topology.nodes = vec![
            super::super::buses::Node {
                path: "/tree".into(),
                label: "/".into(),
                parent: None,
            },
            super::super::buses::Node {
                path: "/tree/soc".into(),
                label: "soc".into(),
                parent: Some(0),
            },
            super::super::buses::Node {
                path: "/tree/soc/uart@1000".into(),
                label: "uart@1000".into(),
                parent: Some(1),
            },
        ];
        let strip = egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(600.0, 42.0));
        for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
            let colors = crate::ui_appearance::WorkbenchColors::from_palette(
                &AppPalette::for_appearance(appearance),
            );
            let context = egui::Context::default();
            let output = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 200.0),
                    )),
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| {
                        buses::paint(ui, strip, &topology, colors);
                    });
                },
            );
            let cells = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Rect(rect) if rect.fill == colors.recessed => {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(cells.len(), 3);
            assert!(
                cells
                    .iter()
                    .all(|cell| strip.contains_rect(*cell) && cell.height() == 42.0)
            );
            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,egui::epaint::Shape::Text(text) if text.galley.job.text.contains("uart@1000"))));
        }
    }
}
