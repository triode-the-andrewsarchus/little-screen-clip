//! Registered I²C/SPI metadata only: no transfers, bus scans or register reads.
mod names;
use super::{
    Document, Relation,
    attributes::{self, Context},
    buses::{self, Registration, Topology},
    collector::Reads,
    model::{Fact, Outcome},
};
use names::Object;
use std::{
    io,
    path::{Path, PathBuf},
};
fn object(path: &Path) -> Option<Object> {
    Object::parse(path.file_name()?.to_str()?, &buses::subsystem(path)?)
}
fn controller(sys: &Path, selected: Object) -> Option<PathBuf> {
    sys.ancestors()
        .take(10)
        .find(|path| object(path).is_some_and(|candidate| selected.controller(candidate)))
        .map(Path::to_path_buf)
}
fn name(document: &mut Document, path: &Path, reads: &mut Reads) -> String {
    attributes::rules(&Context::discover(path))
        .into_iter()
        .filter(|rule| rule.name == "name" || rule.name == "modalias")
        .find_map(|rule| {
            reads
                .observe(document, path, &rule)
                .value
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
}
fn label(document: &mut Document, path: &Path, reads: &mut Reads) -> String {
    let name = name(document, path, reads);
    let location = match object(path) {
        Some(Object::I2cClient(_, address)) => names::address_label(address),
        Some(Object::SpiDevice(_, select) | Object::SpiAccess(_, select)) => {
            format!("Chip select {select}")
        }
        _ => path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
    };
    format!(
        "{location} · {name} · {}",
        buses::driver(path).unwrap_or_else(|| "binding unavailable".into())
    )
}
fn identity(document: &mut Document, sys: &Path, selected: Object) {
    document.title = selected.title().into();
    document.facts.push(Fact::text(
        "Registered bus number",
        selected.bus().to_string(),
        sys.into(),
    ));
    let location = match selected {
        Object::I2cClient(_, address) => {
            Some(("Registered I²C address", names::address_label(address)))
        }
        Object::SpiDevice(_, select) | Object::SpiAccess(_, select) => {
            Some(("Registered chip select", select.to_string()))
        }
        _ => None,
    };
    if let Some((label, value)) = location {
        document.facts.push(Fact::text(label, value, sys.into()));
    }
    if let Some(driver) = buses::driver(sys) {
        document.facts.push(Fact::text(
            "Registered device driver",
            driver,
            sys.join("driver"),
        ));
    }
}
pub(super) fn collect(
    document: &mut Document,
    sys: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let Some(selected) = object(sys) else {
        return;
    };
    identity(document, sys, selected);
    let Some(root) = controller(sys, selected) else {
        document.facts.push(Fact::failure(
            "Registered controller relationship",
            sys.into(),
            io::Error::new(
                io::ErrorKind::NotFound,
                "Matching controller is not exposed in this kernel parent chain",
            ),
        ));
        return;
    };
    let root_name = name(document, &root, reads);
    let summary = if matches!(
        selected,
        Object::I2cClient(..) | Object::SpiDevice(..) | Object::SpiAccess(..)
    ) {
        format!(
            "{} · bus {} · kernel registration",
            label(document, sys, reads),
            selected.bus()
        )
    } else {
        format!(
            "{root_name} · bus {} · kernel registrations",
            selected.bus()
        )
    };
    let mut topology = Topology::new("Registered bus devices", summary);
    let parent = root
        .join("device")
        .canonicalize()
        .ok()
        .or_else(|| root.parent().map(Path::to_path_buf));
    let mut upstream = None;
    if let Some(parent) = parent.filter(|parent| parent != &root) {
        let driver = buses::driver(&parent).unwrap_or_else(|| "binding unavailable".into());
        upstream = Some(buses::add(
            &mut topology,
            document,
            parent.clone(),
            format!(
                "{} · {driver}",
                parent.file_name().unwrap_or_default().to_string_lossy()
            ),
            None,
        ));
        document.facts.push(Fact::text(
            "Adapter/controller parent",
            parent.display().to_string(),
            parent.clone(),
        ));
        if let Some(driver) = buses::driver(&parent) {
            document.facts.push(Fact::text(
                "Adapter parent driver",
                driver,
                parent.join("driver"),
            ));
        }
    }
    let anchor = buses::add(&mut topology, document, root.clone(), root_name, upstream);
    if sys != root {
        let selected_label = label(document, sys, reads);
        buses::add(
            &mut topology,
            document,
            sys.into(),
            selected_label,
            Some(anchor),
        );
    }
    access(document, &root);
    buses::access_node(document, sys, "Bus access node");
    mux(&mut topology, document, &root, anchor);
    document.topology = Some(topology);
    checkpoint(document);
    inventory(document, &root, sys, selected, reads, anchor, checkpoint);
    if matches!(selected, Object::I2cClient(..) | Object::SpiDevice(..)) {
        children(document, sys);
    }
    document.facts.push(Fact::text("Bus inspection policy","Kernel registrations and sysfs relationships only. Unmarked addresses do not establish physical absence. No bus access device, EEPROM/register payload, bus transfers, address probes or device creation is requested.",sys.into()));
    checkpoint(document);
}
fn access(document: &mut Document, root: &Path) {
    for directory in [root.join("i2c-dev"), root.join("spidev")] {
        let (paths, _) = buses::walk(&directory, 1, 16);
        for path in paths {
            buses::access_node(document, &path, "Bus access interface");
        }
    }
}
fn mux(topology: &mut Topology, document: &mut Document, root: &Path, anchor: usize) {
    if let Ok(target) = root.join("mux_device").canonicalize() {
        if matches!(object(&target), Some(Object::I2cClient(..))) {
            let upstream = topology.nodes[anchor].parent;
            let mux = buses::add(
                topology,
                document,
                target.clone(),
                "Upstream I²C mux device".into(),
                upstream,
            );
            topology.nodes[anchor].parent = Some(mux);
            document.facts.push(Fact::text(
                "I²C mux relationship",
                target.display().to_string(),
                root.join("mux_device"),
            ));
        }
    }
}
fn channels(document: &mut Document, root: &Path) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            document
                .facts
                .push(Fact::failure("Mux channel inventory", root.into(), error));
            return;
        }
    };
    let mut coverage = Outcome::Observed;
    for (index, entry) in entries.enumerate().take(129) {
        if index == 128 {
            coverage = Outcome::Truncated;
            break;
        }
        let Ok(entry) = entry else {
            coverage = Outcome::Unsupported;
            continue;
        };
        let name = entry.file_name();
        let Some(channel) = name
            .to_str()
            .and_then(|name| name.strip_prefix("channel-"))
            .and_then(names::number)
        else {
            continue;
        };
        if !entry.file_type().is_ok_and(|kind| kind.is_symlink()) {
            continue;
        }
        let Ok(target) = entry.path().canonicalize() else {
            coverage = Outcome::Disappeared;
            continue;
        };
        if matches!(object(&target), Some(Object::I2cAdapter(_))) {
            document
                .relations
                .push(Relation::new(format!("Mux channel {channel}"), target));
        }
    }
    if coverage != Outcome::Observed {
        let mut fact = Fact::text(
            "Mux channel inventory",
            "Bounded metadata inventory is incomplete",
            root.into(),
        );
        fact.outcome = coverage;
        document.facts.push(fact);
    }
}
fn inventory(
    document: &mut Document,
    root: &Path,
    selected_path: &Path,
    selected: Object,
    reads: &mut Reads,
    anchor: usize,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            document.facts.push(Fact::failure(
                "Registered device inventory",
                root.into(),
                error,
            ));
            if let Some(topology) = &mut document.topology {
                topology.coverage = Outcome::Unsupported;
            }
            return;
        }
    };
    let (paths, coverage) = buses::select_children(
        entries,
        |entry| {
            entry.file_type().is_ok_and(|kind| kind.is_dir())
                && object(&entry.path()).is_some_and(|kind| {
                    matches!(kind, Object::I2cClient(..) | Object::SpiDevice(..))
                        && kind.bus() == selected.bus()
                })
        },
        32,
    );
    let Some(mut topology) = document.topology.take() else {
        return;
    };
    topology.coverage = coverage;
    let mut registrations = Vec::new();
    for path in paths {
        let entry_label = label(document, &path, reads);
        if let Some(Object::I2cClient(_, encoded)) = object(&path) {
            if let Some((address, bits, target)) = names::address(encoded) {
                registrations.push(Registration {
                    address,
                    bits,
                    target,
                    label: entry_label.clone(),
                    path: path.clone(),
                    selected: path == selected_path,
                });
            }
        }
        if !topology.nodes.iter().any(|node| node.path == path) {
            buses::add(
                &mut topology,
                document,
                path.clone(),
                entry_label,
                Some(anchor),
            );
        }
        channels(document, &path);
        buses::access_node(document, &path, "Registered device access");
    }
    if matches!(
        selected,
        Object::I2cAdapter(_) | Object::I2cAccess(_) | Object::I2cClient(..)
    ) {
        topology.addresses = Some(registrations);
    }
    document.topology = Some(topology);
    checkpoint(document);
}
fn children(document: &mut Document, sys: &Path) {
    let (paths, coverage) = buses::select_descendants(sys, 4, 16, |path| {
        matches!(
            buses::subsystem(path).as_deref(),
            Some("hwmon" | "input" | "rtc" | "mtd" | "iio")
        ) && path
            .ancestors()
            .skip(1)
            .find(|ancestor| {
                matches!(
                    object(ancestor),
                    Some(Object::I2cClient(..) | Object::SpiDevice(..))
                )
            })
            .is_some_and(|ancestor| ancestor == sys)
    });
    for path in paths {
        buses::access_node(document, &path, "Related device access");
        document
            .relations
            .push(Relation::new("Related subsystem object", path));
    }
    if coverage != Outcome::Observed {
        let mut fact = Fact::text(
            "Related subsystem inventory",
            "Bounded descendant inventory is incomplete",
            sys.into(),
        );
        fact.outcome = coverage;
        document.facts.push(fact);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, os::unix::fs::symlink};
    fn node(path: &Path, bus: &Path) {
        std::fs::create_dir_all(path).unwrap();
        symlink(bus, path.join("subsystem")).unwrap();
    }
    #[test]
    fn related_subsystems_belong_to_the_nearest_registered_client() {
        let tmp = tempfile::tempdir().unwrap();
        for (subsystem, selected_name, nested_name) in
            [("i2c", "8-0048", "8-0049"), ("spi", "spi8.0", "spi8.1")]
        {
            let bus = tmp.path().join(subsystem);
            let hwmon = tmp.path().join(format!("{subsystem}-classes/hwmon"));
            let input = tmp.path().join(format!("{subsystem}-classes/input"));
            for registration in [&bus, &hwmon, &input] {
                std::fs::create_dir_all(registration).unwrap();
            }
            let selected = tmp.path().join(subsystem).join(selected_name);
            let nested = selected.join("bridge").join(nested_name);
            let sensor = selected.join("hwmon/hwmon0");
            let buttons = selected.join("input/input0");
            let nested_sensor = nested.join("hwmon/hwmon1");
            for (path, class) in [
                (&selected, &bus),
                (&nested, &bus),
                (&sensor, &hwmon),
                (&buttons, &input),
                (&nested_sensor, &hwmon),
            ] {
                node(path, class);
            }
            symlink(&nested_sensor, selected.join("foreign-sensor-alias")).unwrap();
            std::fs::write(
                sensor.join("temp1_input"),
                b"not a requested sensor reading",
            )
            .unwrap();
            let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
            children(&mut document, &selected);
            let targets = document
                .relations
                .iter()
                .filter(|relation| relation.label == "Related subsystem object")
                .map(|relation| &relation.target)
                .collect::<Vec<_>>();
            assert_eq!(targets, vec![&sensor, &buttons]);
            assert!(
                !document
                    .relations
                    .iter()
                    .any(|relation| relation.target == nested_sensor)
            );
        }
    }
    #[test]
    fn related_subsystem_inventory_discloses_missing_and_truncated_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().join("i2c");
        let hwmon = tmp.path().join("hwmon");
        std::fs::create_dir(&bus).unwrap();
        std::fs::create_dir(&hwmon).unwrap();
        let selected = tmp.path().join("8-0048");
        node(&selected, &bus);
        for id in 0..20 {
            node(&selected.join(format!("hwmon{id}")), &hwmon);
        }
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        children(&mut document, &selected);
        assert_eq!(
            document
                .relations
                .iter()
                .filter(|relation| relation.label == "Related subsystem object")
                .count(),
            16
        );
        assert_eq!(document.facts.last().unwrap().outcome, Outcome::Truncated);
        let mut missing = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        children(&mut missing, &tmp.path().join("removed"));
        assert!(
            !missing
                .relations
                .iter()
                .any(|relation| relation.label == "Related subsystem object")
        );
        assert_eq!(missing.facts.last().unwrap().outcome, Outcome::Unsupported);
        channels(&mut missing, &tmp.path().join("removed"));
        let failure = missing.facts.last().unwrap();
        assert_eq!(failure.label, "Mux channel inventory");
        assert_eq!(failure.outcome, Outcome::Missing);
    }
    #[test]
    fn missing_matching_controller_preserves_identity_without_inventing_a_binding() {
        let tmp = tempfile::tempdir().unwrap();
        for (subsystem, controller_class, parent, selected) in [
            ("i2c", "i2c", "i2c-9", "8-0048"),
            ("spi", "spi_master", "spi9", "spi8.3"),
        ] {
            let bus = tmp.path().join(subsystem);
            let class = tmp.path().join(controller_class);
            std::fs::create_dir_all(&bus).unwrap();
            std::fs::create_dir_all(&class).unwrap();
            let parent = tmp.path().join(format!("devices-{subsystem}")).join(parent);
            let selected = parent.join(selected);
            node(&parent, &class);
            node(&selected, &bus);
            let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
            let mut reads = Reads::fixture(BTreeMap::new());
            collect(&mut document, &selected, &mut reads, &mut |_| {});
            assert!(
                document
                    .facts
                    .iter()
                    .any(|fact| fact.label == "Registered bus number"
                        && fact.value.as_deref() == Some("8"))
            );
            assert!(
                document
                    .facts
                    .iter()
                    .any(|fact| fact.label == "Registered controller relationship"
                        && fact.outcome == Outcome::Missing)
            );
            assert!(document.topology.is_none());
            assert!(
                !document
                    .relations
                    .iter()
                    .any(|relation| relation.target == parent)
            );
            assert!(reads.attempts().next().is_none());
        }
    }
    #[test]
    fn logical_i2c_bus_links_mux_channels_and_never_reads_client_payloads() {
        let tmp = tempfile::tempdir().unwrap();
        let bus = tmp.path().join("i2c");
        std::fs::create_dir(&bus).unwrap();
        let upstream = tmp.path().join("controller/i2c-7");
        let mux_device = upstream.join("7-0071");
        let logical = mux_device.join("i2c-73");
        let sensor = logical.join("73-0048");
        for path in [&upstream, &mux_device, &logical, &sensor] {
            node(path, &bus);
        }
        symlink(&upstream, logical.join("device")).unwrap();
        symlink(&mux_device, logical.join("mux_device")).unwrap();
        symlink(&logical, mux_device.join("channel-1")).unwrap();
        std::fs::write(sensor.join("eeprom"), b"must never be read").unwrap();
        let values = BTreeMap::from([
            (upstream.join("name"), b"Upstream".to_vec()),
            (logical.join("name"), b"Logical channel".to_vec()),
            (sensor.join("name"), b"Registered sensor".to_vec()),
            (mux_device.join("name"), b"Mux".to_vec()),
        ]);
        let mut reads = Reads::fixture(values.clone());
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        collect(&mut document, &logical, &mut reads, &mut |_| {});
        let topology = document.topology.as_ref().unwrap();
        let logical_index = topology
            .nodes
            .iter()
            .position(|node| node.path == logical)
            .unwrap();
        assert_eq!(
            topology.nodes[topology.nodes[logical_index].parent.unwrap()].path,
            mux_device
        );
        let registration = &topology.addresses.as_ref().unwrap()[0];
        assert_eq!(registration.address, 0x48);
        assert_eq!(registration.bits, 7);
        assert!(!registration.selected);
        assert!(reads.attempts().all(|path| matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("name" | "modalias")
        )));
        let mut client_document =
            super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        collect(
            &mut client_document,
            &mux_device,
            &mut Reads::fixture(values),
            &mut |_| {},
        );
        assert!(
            client_document
                .relations
                .iter()
                .any(|relation| relation.label == "Mux channel 1" && relation.target == logical)
        );
        assert!(attributes::rule(&Context::discover(&sensor), "eeprom").is_none());
        assert!(attributes::rule(&Context::discover(&logical), "new_device").is_none());
    }
    #[test]
    fn spi_registration_uses_chip_select_and_modalias_without_transfers() {
        let tmp = tempfile::tempdir().unwrap();
        let master = tmp.path().join("spi_master");
        let spi = tmp.path().join("spi");
        let access = tmp.path().join("spidev");
        for path in [&master, &spi, &access] {
            std::fs::create_dir(path).unwrap();
        }
        let controller = tmp.path().join("platform/spi2");
        let device = controller.join("spi2.3");
        let interface = device.join("spidev/spidev2.3");
        node(&controller, &master);
        node(&device, &spi);
        node(&interface, &access);
        let mut reads = Reads::fixture(BTreeMap::from([(
            device.join("modalias"),
            b"spi:flash-name".to_vec(),
        )]));
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        collect(&mut document, &device, &mut reads, &mut |_| {});
        assert_eq!(document.title, "Registered SPI device");
        assert!(
            document
                .topology
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .any(|node| node.label.contains("Chip select 3")
                    && node.label.contains("spi:flash-name"))
        );
        assert!(document.topology.as_ref().unwrap().addresses.is_none());
        assert!(reads.attempts().all(|path| matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("name" | "modalias")
        )));
        let mut interface_document =
            super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        collect(&mut interface_document, &interface, &mut reads, &mut |_| {});
        assert_eq!(interface_document.title, "SPI access interface");
        assert!(interface_document.facts.iter().any(
            |fact| fact.label == "Registered chip select" && fact.value.as_deref() == Some("3")
        ));
    }
}
