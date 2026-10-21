//! Shared passive interface topology and USB identity; no access-node payloads.
use super::{
    Document, Relation,
    attributes::Context,
    collector::Reads,
    model::{Fact, Outcome},
};
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::file_manager) struct Node {
    #[serde(with = "super::path_serde")]
    pub path: PathBuf,
    pub label: String,
    pub parent: Option<usize>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::file_manager) struct Registration {
    pub address: u16,
    pub bits: u8,
    pub target: bool,
    pub label: String,
    #[serde(with = "super::path_serde")]
    pub path: PathBuf,
    pub selected: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::file_manager) struct Topology {
    pub heading: String,
    pub summary: String,
    pub nodes: Vec<Node>,
    pub coverage: Outcome,
    pub retained: bool,
    #[serde(default)]
    pub addresses: Option<Vec<Registration>>,
}
impl Topology {
    pub fn new(heading: &str, summary: String) -> Self {
        Self {
            heading: heading.into(),
            summary,
            nodes: Vec::new(),
            coverage: Outcome::Observed,
            retained: false,
            addresses: None,
        }
    }
    pub(super) fn inventory_failure(
        &mut self,
        document: &mut Document,
        label: &str,
        path: &Path,
        error: io::Error,
    ) {
        let fact = Fact::failure(label, path.into(), error);
        if self.coverage == Outcome::Observed || fact.outcome == Outcome::Truncated {
            self.coverage = fact.outcome.clone();
        }
        document.facts.push(fact);
    }
}
/// Recognizes kernel registration names with a bounded decimal suffix.
pub(super) fn numbered(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix).is_some_and(|value| {
        !value.is_empty()
            && value.bytes().all(|byte| byte.is_ascii_digit())
            && value.parse::<u32>().is_ok()
    })
}
/// Builds a compact summary from the selected object's observed fields.
pub(super) fn attribute_summary(document: &Document, root: &Path) -> String {
    scoped_summary(document, root, |fact| {
        fact.value
            .as_ref()
            .map(|value| format!("{}: {value}", fact.label))
    })
}
/// Shares object naming and field scope while allowing each family to select captions.
pub(super) fn scoped_summary(
    document: &Document,
    root: &Path,
    caption: impl Fn(&Fact) -> Option<String>,
) -> String {
    let mut summary = root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    for fact in document
        .facts
        .iter()
        .filter(|fact| fact.source.parent() == Some(root) && !fact.interface)
    {
        if let Some(value) = caption(fact) {
            summary.push_str(" · ");
            summary.push_str(&value);
            if fact.retained {
                summary.push_str(&format!(" (last observation; {})", fact.outcome));
            } else if fact.outcome != Outcome::Observed {
                summary.push_str(&format!(" ({})", fact.outcome));
            }
        }
    }
    summary
}

#[cfg(test)]
mod summary_tests {
    use super::*;
    #[test]
    fn shared_summary_discloses_retained_and_failed_values_without_changing_source_facts() {
        let root = Path::new("/sys/example/device0");
        let mut doc = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        let mut old = Fact::text("State", "running", root.join("state"));
        old.retained = true;
        old.outcome = Outcome::PermissionDenied;
        let mut failed = Fact::text("Limit", "100", root.join("limit"));
        failed.outcome = Outcome::TimedOut;
        doc.facts = vec![
            old,
            failed,
            Fact::text("Identity", "example", root.join("name")),
        ];
        let summary = attribute_summary(&doc, root);
        assert!(summary.contains("State: running (last observation; Permission denied)"));
        assert!(summary.contains("Limit: 100 (Timed out)"));
        assert!(summary.contains("Identity: example"));
        assert_eq!(doc.facts[0].value.as_deref(), Some("running"));
        assert!(doc.facts[0].retained);
    }
}
pub(super) fn access_node(document: &mut Document, path: &Path, label: &str) {
    if let Ok(device) = udev::Device::from_syspath(path) {
        if let Some(node) = device.devnode() {
            if !document
                .relations
                .iter()
                .any(|relation| relation.target == node)
            {
                document.relations.push(Relation::new(label, node.into()));
            }
        }
    }
}
/// Bounded udev database text with provenance shared by bus and storage collectors.
pub(super) fn database_fact(
    device: &udev::Device,
    path: &Path,
    property: &str,
    label: &str,
) -> Option<Fact> {
    let value = device.property_value(property)?.to_str()?;
    let mut fact = Fact::text(
        label,
        value.chars().take(256).collect::<String>(),
        path.into(),
    );
    fact.origin = format!("udev property {property}");
    if value.chars().count() > 256 {
        fact.outcome = Outcome::Truncated;
    }
    Some(fact)
}
pub(super) fn select_children(
    entries: std::fs::ReadDir,
    predicate: impl Fn(&std::fs::DirEntry) -> bool,
    max_results: usize,
) -> (Vec<PathBuf>, Outcome) {
    let mut paths = Vec::new();
    let mut coverage = Outcome::Observed;
    for (index, entry) in entries.enumerate().take(129) {
        if index == 128 {
            coverage = Outcome::Truncated;
            break;
        }
        match entry {
            Ok(entry) if predicate(&entry) => paths.push(entry.path()),
            Ok(_) => {}
            Err(_) => coverage = Outcome::Unsupported,
        }
    }
    paths.sort();
    if paths.len() > max_results {
        coverage = Outcome::Truncated;
        paths.truncate(max_results);
    }
    (paths, coverage)
}
pub(super) fn walk(root: &Path, max_depth: usize, limit: usize) -> (Vec<PathBuf>, Outcome) {
    let mut pending = vec![(root.to_path_buf(), 0)];
    let mut paths = Vec::new();
    let mut coverage = Outcome::Observed;
    while let Some((path, depth)) = pending.pop() {
        if paths.len() >= limit {
            coverage = Outcome::Truncated;
            break;
        }
        paths.push(path.clone());
        if depth == max_depth {
            continue;
        }
        let entries = match std::fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(_) => {
                coverage = Outcome::Unsupported;
                continue;
            }
        };
        for (index, entry) in entries.enumerate().take(limit + 1) {
            if index == limit {
                coverage = Outcome::Truncated;
                break;
            }
            let Ok(entry) = entry else {
                coverage = Outcome::Unsupported;
                continue;
            };
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            if pending.len() + paths.len() >= limit {
                coverage = Outcome::Truncated;
                break;
            }
            pending.push((entry.path(), depth + 1));
        }
    }
    (paths, coverage)
}
pub(super) fn select_descendants(
    root: &Path,
    depth: usize,
    maximum: usize,
    predicate: impl Fn(&Path) -> bool,
) -> (Vec<PathBuf>, Outcome) {
    let (mut paths, mut coverage) = walk(root, depth, 256);
    paths.retain(|path| predicate(path));
    paths.sort();
    if paths.len() > maximum {
        coverage = Outcome::Truncated;
        paths.truncate(maximum);
    }
    (paths, coverage)
}
/// Selects bounded metadata ancestors in root-to-leaf order for topology chains.
pub(super) fn ancestors(
    root: &Path,
    maximum: usize,
    predicate: impl Fn(&Path) -> bool,
) -> Vec<PathBuf> {
    let mut paths = root
        .ancestors()
        .skip(1)
        .take(maximum)
        .filter(|path| predicate(path))
        .map(Path::to_path_buf)
        .collect::<Vec<_>>();
    paths.reverse();
    paths
}
pub(super) fn ancestor_chain(
    topology: &mut Topology,
    document: &mut Document,
    paths: impl IntoIterator<Item = PathBuf>,
    mut parent: Option<usize>,
) -> Option<usize> {
    for path in paths {
        let mut label = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if let Some(driver) = driver(&path) {
            label.push_str(&format!(" · {driver}"));
        }
        parent = Some(add(topology, document, path, label, parent));
    }
    parent
}
/// Adds declared metadata symlinks without opening linked content.
pub(super) fn link_relations(document: &mut Document, root: &Path, names: &[&str], prefix: &str) {
    for name in names {
        if std::fs::symlink_metadata(root.join(name))
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            if let Ok(target) = root.join(name).canonicalize() {
                document
                    .relations
                    .push(Relation::new(format!("{prefix} {name}"), target));
            }
        }
    }
}
pub(super) fn subsystem(path: &Path) -> Option<String> {
    path.join("subsystem")
        .canonicalize()
        .ok()?
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}
fn serial_name(name: &str) -> bool {
    ["ttyUSB", "ttyACM"].iter().any(|prefix| {
        name.strip_prefix(prefix)
            .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()))
    })
}
pub(super) fn usb_parent(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .take(10)
        .find(|path| subsystem(path).as_deref() == Some("usb") && path.join("idVendor").is_file())
        .map(Path::to_path_buf)
}

pub(super) fn directory_group(sys: &Path) -> Option<(Option<PathBuf>, String)> {
    if subsystem(sys).as_deref() != Some("usb") {
        return None;
    }
    let device = usb_parent(sys)?;
    let bus = device.ancestors().take(10).find(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| numbered(name, "usb"))
            && subsystem(path).as_deref() == Some("usb")
    })?;
    Some((
        Some(bus.into()),
        format!("USB device {}", device.file_name()?.to_string_lossy()),
    ))
}

/// Directory captions collect only the two standard identity words, never descriptors/endpoints.
pub(super) fn directory_identity(
    document: &mut Document,
    sys: &Path,
    reads: &mut Reads,
    topology: &mut Topology,
) -> String {
    let context = Context::discover(sys);
    let mut ids = Vec::new();
    for name in ["idVendor", "idProduct"] {
        let Some(rule) = super::attributes::rule(&context, name) else {
            continue;
        };
        let mut fact = reads.fact(sys, &rule);
        ids.push(
            fact.raw
                .clone()
                .filter(|_| fact.outcome == Outcome::Observed)
                .unwrap_or_else(|| "unavailable".into()),
        );
        if fact.outcome != Outcome::Observed && topology.coverage == Outcome::Observed {
            topology.coverage = fact.outcome.clone();
        }
        fact.label = format!(
            "{} · {}",
            sys.file_name().unwrap_or_default().to_string_lossy(),
            fact.label
        );
        document.facts.push(fact);
    }
    ids.join(":")
}
pub(super) fn driver(path: &Path) -> Option<String> {
    path.join("driver")
        .canonicalize()
        .ok()?
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
}
pub(super) fn add(
    topology: &mut Topology,
    document: &mut Document,
    path: PathBuf,
    label: String,
    parent: Option<usize>,
) -> usize {
    add_relation(topology, document, Relation::new(label, path), parent)
}
/// Adds a captured relationship without replacing its observed target identity.
pub(super) fn add_relation(
    topology: &mut Topology,
    document: &mut Document,
    relation: Relation,
    parent: Option<usize>,
) -> usize {
    let index = topology.nodes.len();
    topology.nodes.push(Node {
        path: relation.target.clone(),
        label: relation.label.clone(),
        parent,
    });
    document.relations.push(relation);
    index
}
/// Connects a discovered descendant to its nearest represented ancestor.
pub(super) fn add_descendant(
    topology: &mut Topology,
    document: &mut Document,
    path: PathBuf,
    anchor: usize,
) -> usize {
    let parent = path
        .ancestors()
        .skip(1)
        .find_map(|ancestor| {
            topology
                .nodes
                .iter()
                .rposition(|node| node.path == ancestor)
        })
        .unwrap_or(anchor);
    let label = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    add(topology, document, path, label, Some(parent))
}
pub(super) fn collect(document: &mut Document, sys: &Path, reads: &mut Reads) {
    let leaf = sys.file_name().unwrap_or_default().to_string_lossy();
    let serial = serial_name(&leaf);
    if !serial && subsystem(sys).as_deref() != Some("usb") {
        return;
    }
    let Some(device) = usb_parent(sys) else {
        return;
    };
    let product = document
        .observed_attribute(&device, "product")
        .and_then(|fact| fact.raw.as_deref())
        .unwrap_or("USB device");
    let ids = match (
        document
            .observed_attribute(&device, "idVendor")
            .and_then(|fact| fact.raw.as_deref()),
        document
            .observed_attribute(&device, "idProduct")
            .and_then(|fact| fact.raw.as_deref()),
    ) {
        (Some(vendor), Some(product)) => format!("{vendor}:{product}"),
        _ => "VID:PID unavailable".into(),
    };
    let speed = document
        .observed_attribute(&device, "speed")
        .and_then(|fact| fact.value.as_deref())
        .unwrap_or("speed unavailable");
    let mut topology = Topology::new("USB topology", format!("{product} · {ids} · {speed}"));
    let mut parents = device
        .ancestors()
        .take(8)
        .filter(|path| subsystem(path).as_deref() == Some("usb"))
        .map(Path::to_path_buf)
        .collect::<Vec<_>>();
    parents.reverse();
    let mut parent = None;
    if let Some(controller) = parents
        .first()
        .and_then(|root| root.parent())
        .filter(|path| subsystem(path).as_deref() == Some("pci"))
    {
        let bound = driver(controller).unwrap_or_else(|| "binding unavailable".into());
        parent = Some(add(
            &mut topology,
            document,
            controller.into(),
            format!(
                "{} · {bound}",
                controller.file_name().unwrap_or_default().to_string_lossy()
            ),
            None,
        ));
    }
    let root = ancestor_chain(&mut topology, document, parents, parent);
    interfaces(&mut topology, document, &device, reads, root);
    if serial {
        let bound = sys
            .ancestors()
            .take(6)
            .find_map(driver)
            .unwrap_or_else(|| "driver unavailable".into());
        topology.summary = format!("USB serial · {} · {bound}", topology.summary);
        let related = topology
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| sys.starts_with(&node.path) && node.path != device)
            .max_by_key(|(_, node)| node.path.components().count())
            .map(|(index, _)| index);
        add(
            &mut topology,
            document,
            sys.into(),
            format!("{leaf} · {bound}"),
            related.or(root),
        );
        document.facts.push(Fact::text(
            "Serial configuration",
            "Not queried; USB-facing identity does not establish downstream hardware or baud rate",
            sys.into(),
        ));
        document.title = "USB serial interface".into();
    } else {
        document.title = if sys.join("bInterfaceNumber").is_file() {
            "USB interface"
        } else {
            "USB device"
        }
        .into();
    }
    document.facts.push(Fact::text(
        "Physical USB port path",
        device.file_name().unwrap_or_default().to_string_lossy(),
        device.clone(),
    ));
    document.topology = Some(topology);
}
const USB_INTERFACE_FIELDS: &[&str] = &[
    "bInterfaceNumber",
    "bAlternateSetting",
    "bInterfaceClass",
    "bInterfaceSubClass",
    "bInterfaceProtocol",
    "bNumEndpoints",
];
/// Device and serial views expose each interface's own descriptor scope.
fn interface_details(
    document: &mut Document,
    path: &Path,
    reads: &mut Reads,
    topology: &mut Topology,
) {
    let context = Context::discover(path);
    for mut rule in super::attributes::rules(&context)
        .into_iter()
        .filter(|rule| USB_INTERFACE_FIELDS.contains(&rule.name.as_str()))
    {
        if document.path != path {
            rule.label = format!(
                "{} · {}",
                path.file_name().unwrap_or_default().to_string_lossy(),
                rule.label
            );
        }
        let fact = reads.observe(document, path, &rule);
        if topology.coverage == Outcome::Observed
            && fact.outcome != Outcome::Observed
            && fact.outcome != Outcome::Missing
        {
            topology.coverage = fact.outcome;
        }
    }
}

fn interfaces(
    topology: &mut Topology,
    document: &mut Document,
    device: &Path,
    reads: &mut Reads,
    parent: Option<usize>,
) {
    let entries = match std::fs::read_dir(device) {
        Ok(entries) => entries,
        Err(error) => {
            topology.inventory_failure(document, "USB interface inventory", device, error);
            return;
        }
    };
    let (paths, coverage) = select_children(
        entries,
        |entry| {
            subsystem(&entry.path()).as_deref() == Some("usb")
                && entry.path().join("bInterfaceNumber").is_file()
        },
        8,
    );
    if coverage != Outcome::Observed
        && (topology.coverage == Outcome::Observed || coverage == Outcome::Truncated)
    {
        topology.coverage = coverage;
    }
    for path in paths {
        interface_details(document, &path, reads, topology);
        let class = reads
            .read(&path.join("bInterfaceClass"))
            .ok()
            .map(|value| interface_class(&value))
            .unwrap_or_else(|| "class unavailable".into());
        let bound = driver(&path).unwrap_or_else(|| "binding unavailable".into());
        let label = format!(
            "{} · {class} · {bound}",
            path.file_name().unwrap_or_default().to_string_lossy()
        );
        let interface = add(topology, document, path.clone(), label, parent);
        access_nodes(document, &path);
        endpoints(topology, document, &path, reads, interface);
    }
}
fn access_nodes(document: &mut Document, interface: &Path) {
    let (paths, coverage) = walk(interface, 3, 128);
    for path in paths {
        access_node(document, &path, "USB access interface");
    }
    if coverage != Outcome::Observed {
        let mut fact = Fact::text(
            "USB access-interface inventory",
            "Bounded descendant inventory is incomplete",
            interface.into(),
        );
        fact.outcome = coverage;
        document.facts.push(fact);
    }
}
fn endpoints(
    topology: &mut Topology,
    document: &mut Document,
    interface: &Path,
    reads: &mut Reads,
    parent: usize,
) {
    let entries = match std::fs::read_dir(interface) {
        Ok(entries) => entries,
        Err(error) => {
            topology.inventory_failure(document, "USB endpoint inventory", interface, error);
            return;
        }
    };
    for (index, entry) in entries.enumerate().take(65) {
        if index == 64 {
            topology.inventory_failure(
                document,
                "USB endpoint inventory",
                interface,
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Endpoint inventory capped at 64 immediate entries",
                ),
            );
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                topology.inventory_failure(document, "USB endpoint inventory", interface, error);
                continue;
            }
        };
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("ep_") {
            continue;
        }
        match std::fs::metadata(entry.path()) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => continue,
            Err(error) => {
                topology.inventory_failure(
                    document,
                    "USB endpoint membership",
                    &entry.path(),
                    error,
                );
                continue;
            }
        }
        if topology.nodes.len() >= 40 {
            topology.inventory_failure(
                document,
                "USB endpoint inventory",
                interface,
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "USB topology capped at 40 nodes",
                ),
            );
            break;
        }
        let label = endpoint_details(topology, document, &entry.path(), reads);
        add(topology, document, entry.path(), label, Some(parent));
    }
}
/// Shares typed descriptor facts and the derived endpoint caption.
fn endpoint_details(
    topology: &mut Topology,
    document: &mut Document,
    path: &Path,
    reads: &mut Reads,
) -> String {
    let context = Context::discover(path);
    for rule in super::attributes::rules(&context) {
        let fact = reads.fact(path, &rule);
        if topology.coverage == Outcome::Observed
            && fact.outcome != Outcome::Observed
            && fact.outcome != Outcome::Missing
        {
            topology.coverage = fact.outcome.clone();
        }
        document.facts.push(fact);
    }
    let description = (|| {
        endpoint(
            &reads.read(&path.join("bEndpointAddress"))?,
            &reads.read(&path.join("bmAttributes"))?,
            &reads.read(&path.join("wMaxPacketSize"))?,
        )
    })();
    match description {
        Ok(label) => label,
        Err(error) => {
            topology.inventory_failure(document, "USB endpoint layout", path, error);
            "Endpoint metadata unavailable".into()
        }
    }
}
fn interface_class(raw: &str) -> String {
    match u8::from_str_radix(raw.trim_start_matches("0x"), 16) {
        Ok(1) => "Audio".into(),
        Ok(2) => "Communications".into(),
        Ok(3) => "HID".into(),
        Ok(8) => "Mass storage".into(),
        Ok(9) => "Hub".into(),
        Ok(10) => "CDC data".into(),
        Ok(14) => "Video".into(),
        Ok(255) => "Vendor-specific".into(),
        _ => format!("class {raw}"),
    }
}
fn endpoint(address: &str, attributes: &str, packet: &str) -> io::Result<String> {
    let parse = |value: &str| {
        u16::from_str_radix(value.trim_start_matches("0x"), 16)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid USB endpoint field"))
    };
    let address = parse(address)?;
    let attributes = parse(attributes)?;
    let packet = parse(packet)?;
    if address > 255 || address & 0x70 != 0 || attributes > 255 || packet & 0x7ff == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid USB endpoint byte",
        ));
    }
    let transfer = ["Control", "Isochronous", "Bulk", "Interrupt"][(attributes & 3) as usize];
    Ok(format!(
        "EP {:02x} · {} {transfer} · {} B packet",
        address,
        if address & 0x0f == 0 && attributes & 3 == 0 {
            "bidirectional"
        } else if address & 128 != 0 {
            "IN"
        } else {
            "OUT"
        },
        packet & 0x7ff
    ))
}
pub(super) fn paint(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    topology: &Topology,
    colors: crate::ui_appearance::WorkbenchColors,
) {
    if let Some(registrations) = &topology.addresses {
        paint_addresses(ui, rect, registrations, topology, colors);
        return;
    }
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let visible = topology.nodes.len().min(6).max(1);
    let width = rect.width() / visible as f32;
    for (index, node) in topology.nodes.iter().take(6).enumerate() {
        let cell = egui::Rect::from_min_size(
            egui::pos2(rect.left() + index as f32 * width, rect.top()),
            egui::vec2(width - 5.0, rect.height()),
        );
        painter.rect_filled(
            cell,
            3.0,
            if topology.retained {
                colors.panel
            } else {
                colors.recessed
            },
        );
        let mut label = egui::text::LayoutJob::simple(
            node.label.clone(),
            egui::FontId::proportional(11.0),
            colors.text_secondary,
            (cell.width() - 6.0).max(1.0),
        );
        label.wrap.max_rows = 1;
        label.wrap.break_anywhere = true;
        let label = ui.fonts(|fonts| fonts.layout_job(label));
        painter.with_clip_rect(cell.shrink(3.0)).galley(
            cell.center() - label.size() / 2.0,
            label,
            colors.text_secondary,
        );
        if !crate::file_manager::ui::popup_open(ui.ctx()) {
            ui.interact(
                cell,
                ui.id().with(("bus-topology-node", &node.path)),
                egui::Sense::hover(),
            )
            .on_hover_text(format!(
                "{}\n{}\nParent: {}\nCoverage: {} · {}/{} nodes shown{}",
                node.label,
                node.path.display(),
                node.parent
                    .and_then(|parent| topology.nodes.get(parent))
                    .map(|parent| parent.label.as_str())
                    .unwrap_or("Root"),
                topology.coverage,
                visible,
                topology.nodes.len(),
                if topology.retained {
                    " · last observation"
                } else {
                    ""
                }
            ));
        }
    }
}
fn registration_domains(registrations: &[Registration]) -> Vec<u8> {
    if registrations.is_empty() {
        return vec![7];
    }
    [7, 10]
        .into_iter()
        .filter(|bits| registrations.iter().any(|entry| entry.bits == *bits))
        .collect()
}
fn paint_addresses(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    registrations: &[Registration],
    topology: &Topology,
    colors: crate::ui_appearance::WorkbenchColors,
) {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let domains = registration_domains(registrations);
    for (lane, bits) in domains.iter().enumerate() {
        let stride = if *bits == 7 { 1 } else { 8 };
        let height = rect.height() / domains.len() as f32 / 4.0;
        let width = rect.width() / 32.0;
        for bin in 0..128u16 {
            let start = bin * stride;
            let end = start + stride - 1;
            let matches = |entry: &&Registration| {
                entry.bits == *bits && (start..=end).contains(&entry.address)
            };
            let observed = registrations.iter().filter(matches).collect::<Vec<_>>();
            let cell = egui::Rect::from_min_size(
                egui::pos2(
                    rect.left() + f32::from(bin % 32) * width,
                    rect.top() + (lane * 4) as f32 * height + f32::from(bin / 32) * height,
                ),
                egui::vec2((width - 1.0).max(0.0), (height - 1.0).max(0.0)),
            );
            painter.rect_filled(
                cell,
                0.0,
                if observed.is_empty() || topology.retained {
                    colors.recessed
                } else {
                    colors.accent
                },
            );
            if observed.iter().any(|entry| entry.selected) {
                painter.rect_stroke(cell, 0.0, egui::Stroke::new(1.5_f32, colors.text_primary));
            }
            if !crate::file_manager::ui::popup_open(ui.ctx()) {
                ui.interact(cell,ui.id().with(("registered-address",bits,bin)),egui::Sense::hover()).on_hover_ui(|ui| {
                    ui.label(format!("{bits}-bit registered-address metadata: 0x{start:02x}–0x{end:02x}"));
                    for entry in &observed { ui.label(format!("{}{}\n{}",entry.label,if entry.target {" · target interface"} else {""},entry.path.display())); }
                    if observed.is_empty() {ui.label("No registration shown in this observation; physical absence is not established.");}
                    ui.label(format!("Coverage: {}{}",topology.coverage,if topology.retained {" · last observation"} else {""}));
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoint_and_class_interpretation_does_not_guess_serial_configuration() {
        assert_eq!(
            endpoint("81", "02", "0200").unwrap(),
            "EP 81 · IN Bulk · 512 B packet"
        );
        assert_eq!(
            endpoint("03", "03", "0040").unwrap(),
            "EP 03 · OUT Interrupt · 64 B packet"
        );
        assert!(endpoint("100", "00", "40").is_err());
        assert_eq!(interface_class("03"), "HID");
        assert!(serial_name("ttyUSB7"));
        assert!(serial_name("ttyACM0"));
        assert!(!serial_name("ttyUSB"));
        assert!(!serial_name("tty0"));
    }
}

#[cfg(test)]
mod fixture_tests {
    use super::*;
    use std::{collections::BTreeMap, os::unix::fs::symlink};
    #[test]
    fn usb_inventory_failures_keep_their_types_and_entry_caps_are_disclosed() {
        let temp = tempfile::tempdir().unwrap();
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        document.facts.clear();
        let mut reads = Reads::fixture(BTreeMap::new());
        let mut topology = Topology::new("USB topology", String::new());
        interfaces(
            &mut topology,
            &mut document,
            &temp.path().join("missing"),
            &mut reads,
            None,
        );
        assert_eq!(topology.coverage, Outcome::Missing);
        assert_eq!(document.facts.last().unwrap().outcome, Outcome::Missing);
        let file = temp.path().join("file");
        std::fs::write(&file, b"ordinary fixture").unwrap();
        topology.coverage = Outcome::Observed;
        interfaces(&mut topology, &mut document, &file, &mut reads, None);
        assert_eq!(topology.coverage, Outcome::Unsupported);
        topology.coverage = Outcome::Observed;
        endpoints(
            &mut topology,
            &mut document,
            &temp.path().join("missing"),
            &mut reads,
            0,
        );
        assert_eq!(topology.coverage, Outcome::Missing);
        for index in 0..65 {
            std::fs::write(temp.path().join(format!("attribute{index}")), b"unread").unwrap();
        }
        topology.coverage = Outcome::Observed;
        endpoints(&mut topology, &mut document, temp.path(), &mut reads, 0);
        assert_eq!(topology.coverage, Outcome::Truncated);
        assert!(document.facts.iter().any(
            |fact| fact.label == "USB endpoint inventory" && fact.outcome == Outcome::Truncated
        ));
        assert_eq!(reads.attempts().count(), 0);
    }

    #[test]
    fn endpoint_descriptor_failures_do_not_invent_truncation_or_erase_a_known_cap() {
        let (_temp, endpoint, mut reads, mut document) =
            super::super::collector::registered_fixture(
                "usb",
                "ep_81",
                &[
                    ("bEndpointAddress", "81"),
                    ("bmAttributes", "01"),
                    ("wMaxPacketSize", "0c00"),
                    ("bInterval", "01"),
                    ("interval", "125us"),
                ],
            );
        let mut topology = Topology::new("USB topology", String::new());
        let label = endpoint_details(&mut topology, &mut document, &endpoint, &mut reads);
        assert_eq!(label, "EP 81 · IN Isochronous · 1024 B packet");
        assert_eq!(topology.coverage, Outcome::Observed);
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "Polling interval"
                    && fact.value.as_deref() == Some("125us"))
        );
        reads = Reads::fixture(BTreeMap::new());
        document.facts.clear();
        endpoint_details(&mut topology, &mut document, &endpoint, &mut reads);
        assert_eq!(topology.coverage, Outcome::Missing);
        assert!(document.facts.iter().any(|fact| fact.label == "USB endpoint layout" && fact.outcome == Outcome::Missing));
        reads = Reads::fixture(BTreeMap::from([(
            endpoint.join("bEndpointAddress"),
            vec![255],
        )]));
        document.facts.clear();
        topology.coverage = Outcome::Truncated;
        endpoint_details(&mut topology, &mut document, &endpoint, &mut reads);
        assert_eq!(topology.coverage, Outcome::Truncated);
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.outcome == Outcome::Malformed)
        );
    }
    #[test]
    fn usb_interface_details_are_scoped_and_preserve_failures_without_duplicate_reads() {
        let temp = tempfile::tempdir().unwrap();
        let bus = temp.path().join("usb");
        let first = temp.path().join("1-3:1.0");
        let second = temp.path().join("1-3:1.1");
        std::fs::create_dir(&bus).unwrap();
        for path in [&first, &second] {
            std::fs::create_dir(path).unwrap();
            symlink(&bus, path.join("subsystem")).unwrap();
        }
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        document.path = temp.path().join("1-3");
        document.facts.clear();
        let mut reads = Reads::fixture(BTreeMap::from([
            (first.join("bInterfaceNumber"), b"00".to_vec()),
            (first.join("bAlternateSetting"), b"1".to_vec()),
            (first.join("bInterfaceSubClass"), b"02".to_vec()),
            (first.join("bInterfaceProtocol"), b"01".to_vec()),
            (second.join("bInterfaceNumber"), b"01".to_vec()),
            (second.join("bInterfaceProtocol"), vec![255]),
        ]));
        let mut topology = Topology::new("USB topology", String::new());
        interface_details(&mut document, &first, &mut reads, &mut topology);
        interface_details(&mut document, &second, &mut reads, &mut topology);
        let count = document.facts.len();
        interface_details(&mut document, &first, &mut reads, &mut topology);
        assert_eq!(document.facts.len(), count);
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "1-3:1.0 · Alternate setting"
                    && fact.value.as_deref() == Some("1"))
        );
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.source == second.join("bInterfaceProtocol")
                    && fact.outcome == Outcome::Malformed)
        );
        assert_eq!(topology.coverage, Outcome::Malformed);
        assert!(reads.attempts().all(|path| {
            USB_INTERFACE_FIELDS.contains(&path.file_name().unwrap().to_str().unwrap())
        }));
    }

    #[test]
    fn usb_summary_uses_current_attributes_of_the_selected_device_only() {
        let (_temp, device, mut reads, mut original) = super::super::collector::registered_fixture(
            "usb",
            "1-3",
            &[
                ("idVendor", "10c4"),
                ("idProduct", "ea60"),
                ("product", "Test adapter"),
                ("speed", "480"),
            ],
        );
        original.path = device.clone();
        original.facts.clear();
        let rules = super::super::attributes::rules(&Context::discover(&device));
        for root in [device.parent().unwrap(), device.as_path()] {
            for name in ["product", "idVendor", "idProduct", "speed"] {
                let rule = rules.iter().find(|rule| rule.name == name).unwrap();
                let mut fact = reads.fact(&device, rule);
                fact.source = root.join(name);
                if root != device {
                    fact.raw = Some("upstream".into());
                    fact.value = Some("Upstream hub value".into());
                }
                original.facts.push(fact);
            }
        }
        for state in ["current", "missing", "retained", "failed"] {
            let mut document = original.clone();
            for fact in document
                .facts
                .iter_mut()
                .filter(|fact| fact.source.parent() == Some(device.as_path()))
            {
                match state {
                    "missing" => {
                        fact.raw = None;
                        fact.value = None;
                        fact.outcome = Outcome::Missing;
                    }
                    "retained" => {
                        fact.retained = true;
                        fact.outcome = Outcome::PermissionDenied;
                    }
                    "failed" => {
                        fact.outcome = Outcome::PermissionDenied;
                    }
                    _ => {}
                }
            }
            collect(&mut document, &device, &mut reads);
            let caption = &document.topology.as_ref().unwrap().summary;
            assert!(
                !caption.contains("upstream") && !caption.contains("Upstream"),
                "{state}: {caption}"
            );
            if state == "current" {
                assert_eq!(caption, "Test adapter · 10c4:ea60 · 480 Mb/s");
            } else {
                assert_eq!(
                    caption, "USB device · VID:PID unavailable · speed unavailable",
                    "{state}"
                );
            }
            assert_eq!(
                document.facts.iter().filter(|fact| fact.retained).count(),
                if state == "retained" { 4 } else { 0 }
            );
        }
    }

    #[test]
    fn serial_topology_uses_usb_metadata_and_never_requests_port_contents() {
        let temp = tempfile::tempdir().unwrap();
        let bus = temp.path().join("usb");
        let device = temp.path().join("1-3");
        let interface = device.join("1-3:1.0");
        let serial = interface.join("ttyUSB0");
        std::fs::create_dir(&bus).unwrap();
        std::fs::create_dir_all(&serial).unwrap();
        symlink(&bus, device.join("subsystem")).unwrap();
        symlink(&bus, interface.join("subsystem")).unwrap();
        std::fs::write(device.join("idVendor"), b"marker only").unwrap();
        std::fs::write(interface.join("bInterfaceNumber"), b"marker only").unwrap();
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        document.path = serial.clone();
        document.facts.clear();
        for (name, label, value) in [
            ("product", "USB product", "Test adapter"),
            ("idVendor", "USB vendor ID", "10c4"),
            ("idProduct", "USB product ID", "ea60"),
        ] {
            document
                .facts
                .push(Fact::text(label, value, device.join(name)));
        }
        let mut reads = Reads::fixture(BTreeMap::from([(
            interface.join("bInterfaceClass"),
            b"ff".to_vec(),
        )]));
        collect(&mut document, &serial, &mut reads);
        let topology = document.topology.as_ref().unwrap();
        assert!(topology.summary.contains("USB serial"));
        assert!(topology.summary.contains("10c4:ea60"));
        assert!(reads.attempts().all(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| USB_INTERFACE_FIELDS.contains(&name))
        }));
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "Serial configuration"
                    && fact.value.as_ref().unwrap().contains("Not queried"))
        );
        let port = topology.nodes.last().unwrap();
        let parent = &topology.nodes[port.parent.unwrap()];
        assert_eq!(parent.path, interface);
    }
}
