//! Module and driver registration metadata; parameters remain metadata-only.
use super::{
    Document, Relation,
    buses::{self, Topology},
    model::{Fact, Outcome},
};
use std::{
    io,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

pub(super) fn collect(document: &mut Document, sys: &Path, checkpoint: &mut dyn FnMut(&Document)) {
    collect_at(document, sys, Path::new("/sys/module"), checkpoint);
}

fn collect_at(
    document: &mut Document,
    sys: &Path,
    modules: &Path,
    checkpoint: &mut dyn FnMut(&Document),
) {
    if sys.parent() == Some(modules) {
        module(document, sys, modules, checkpoint);
    } else if let Some(bus) = driver_bus(sys) {
        driver(document, sys, &bus, modules, checkpoint);
    } else if matches!(
        sys.file_name().and_then(|name| name.to_str()),
        Some("parameters" | "holders" | "drivers")
    ) && sys.parent().and_then(Path::parent) == Some(modules)
    {
        module(document, sys.parent().unwrap(), modules, checkpoint);
    }
}

fn driver_bus(path: &Path) -> Option<String> {
    let drivers = path.parent()?;
    if drivers.file_name()? != "drivers" {
        return None;
    }
    let bus = drivers.parent()?;
    (bus.parent()?.file_name()? == "bus")
        .then(|| bus.file_name().unwrap().to_string_lossy().into_owned())
}

fn inventory(
    document: &mut Document,
    topology: &mut Topology,
    path: &Path,
    label: &str,
) -> Vec<PathBuf> {
    match std::fs::read_dir(path) {
        Ok(entries) => {
            let (paths, coverage) = buses::select_children(entries, |_| true, 32);
            if coverage != Outcome::Observed {
                topology.coverage = coverage.clone();
            }
            let mut fact = Fact::text(
                label,
                format!("{} observed entries", paths.len()),
                path.into(),
            );
            fact.outcome = coverage;
            document.facts.push(fact);
            paths
        }
        Err(error) => {
            let fact = Fact::failure(label, path.into(), error);
            // Optional inventories may not be created by the kernel.
            if fact.outcome != Outcome::Missing {
                topology.coverage = fact.outcome.clone();
            }
            document.facts.push(fact);
            Vec::new()
        }
    }
}

fn resolve(
    document: &mut Document,
    topology: &mut Topology,
    path: &Path,
    label: &str,
) -> Option<PathBuf> {
    match path.canonicalize() {
        Ok(target) => Some(target),
        Err(error) => {
            let mut fact = Fact::failure(label, path.into(), error);
            if fact.outcome == Outcome::Missing {
                fact.outcome = Outcome::Disappeared;
            }
            topology.coverage = fact.outcome.clone();
            document.facts.push(fact);
            None
        }
    }
}

fn parameters(
    document: &mut Document,
    topology: &mut Topology,
    root: &Path,
    checkpoint: &mut dyn FnMut(&Document),
) -> usize {
    let paths = inventory(
        document,
        topology,
        &root.join("parameters"),
        "Module parameter inventory",
    );
    let mut count = 0;
    for path in paths {
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                let mut fact = Fact::text(
                    format!("Parameter {name}"),
                    format!(
                        "Permissions {:04o}; value not read (driver-specific callback)",
                        metadata.permissions().mode() & 0o7777
                    ),
                    path.clone(),
                );
                fact.outcome = Outcome::NotRequested;
                document.facts.push(fact);
                document
                    .relations
                    .push(Relation::new(format!("Module parameter {name}"), path));
                count += 1;
            }
            Ok(_) => {}
            Err(error) => {
                document
                    .facts
                    .push(Fact::failure("Module parameter metadata", path, error))
            }
        }
        checkpoint(document);
    }
    count
}

fn module(
    document: &mut Document,
    root: &Path,
    modules: &Path,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let name = root.file_name().unwrap_or_default().to_string_lossy();
    document.title = "Kernel module object".into();
    document
        .facts
        .insert(0, Fact::text("Module name", name.to_string(), root.into()));
    document.facts.insert(1, Fact::text("Module presence", "Registered in this kernel view; missing reference count does not establish built-in status or unloadability", root.into()));
    let mut topology = Topology::new("Module/driver relationships", name.to_string());
    let anchor = buses::add(
        &mut topology,
        document,
        root.into(),
        format!("Module {name}"),
        None,
    );
    let count = parameters(document, &mut topology, root, checkpoint);
    topology
        .summary
        .push_str(&format!(" · {count} observed parameters"));
    for path in inventory(
        document,
        &mut topology,
        &root.join("holders"),
        "Module holder inventory",
    ) {
        match resolve(document, &mut topology, &path, "Module holder") {
            Some(target) if target.parent() == Some(modules) && target != root => {
                let name = target.file_name().unwrap_or_default().to_string_lossy();
                buses::add(
                    &mut topology,
                    document,
                    target.clone(),
                    format!("Dependent module {name} (holder)"),
                    Some(anchor),
                );
            }
            _ => {}
        }
        checkpoint(document);
    }
    dependencies(document, &mut topology, root, modules, anchor, checkpoint);
    for path in inventory(
        document,
        &mut topology,
        &root.join("drivers"),
        "Module driver inventory",
    ) {
        if let Some(target) = resolve(document, &mut topology, &path, "Module driver") {
            if let Some(bus) = driver_bus(&target) {
                let label = format!(
                    "{bus} driver {}",
                    target.file_name().unwrap_or_default().to_string_lossy()
                );
                let parent =
                    buses::add(&mut topology, document, target.clone(), label, Some(anchor));
                bound(document, &mut topology, &target, parent, checkpoint);
            }
        }
        checkpoint(document);
    }
    document.topology = Some(topology);
}

fn dependencies(
    document: &mut Document,
    topology: &mut Topology,
    root: &Path,
    modules: &Path,
    anchor: usize,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let entries = match std::fs::read_dir(modules) {
        Ok(entries) => entries,
        Err(error) => {
            document.facts.push(Fact::failure(
                "Module dependency inventory",
                modules.into(),
                error,
            ));
            topology.coverage = Outcome::Unsupported;
            return;
        }
    };
    let name = root.file_name().unwrap_or_default();
    let mut count = 0;
    let mut coverage = Outcome::Observed;
    for (index, entry) in entries.enumerate().take(513) {
        if index == 512 || count == 32 {
            coverage = Outcome::Truncated;
            break;
        }
        match entry {
            Ok(entry) => {
                let link = entry.path().join("holders").join(name);
                match link.canonicalize() {
                    Ok(target) if target == root => {
                        let label = format!(
                            "Dependency {} (observed holder backlink)",
                            entry.file_name().to_string_lossy()
                        );
                        buses::add(topology, document, entry.path(), label, Some(anchor));
                        count += 1;
                        checkpoint(document);
                    }
                    Err(error) if error.kind() != io::ErrorKind::NotFound => {
                        coverage = Outcome::Unsupported
                    }
                    _ => {}
                }
            }
            Err(_) => coverage = Outcome::Unsupported,
        }
    }
    if coverage != Outcome::Observed {
        topology.coverage = coverage.clone();
    }
    let mut fact = Fact::text(
        "Module dependency inventory",
        format!("{count} observed holder backlinks; not a complete build-time dependency list"),
        modules.into(),
    );
    fact.outcome = coverage;
    document.facts.push(fact);
}

fn driver(
    document: &mut Document,
    root: &Path,
    bus: &str,
    modules: &Path,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let name = root.file_name().unwrap_or_default().to_string_lossy();
    document.title = "Kernel driver".into();
    document
        .facts
        .insert(0, Fact::text("Driver name", name.to_string(), root.into()));
    document.facts.insert(
        1,
        Fact::text(
            "Driver bus",
            bus,
            root.parent().unwrap().parent().unwrap().into(),
        ),
    );
    let mut topology = Topology::new("Driver bindings", format!("{name} · {bus} driver"));
    let anchor = buses::add(
        &mut topology,
        document,
        root.into(),
        format!("{bus} driver {name}"),
        None,
    );
    match root.join("module").canonicalize() {
        Ok(module) if module.parent() == Some(modules) => {
            let name = module.file_name().unwrap_or_default().to_string_lossy();
            document.facts.push(Fact::text(
                "Associated module",
                name.to_string(),
                root.join("module"),
            ));
            buses::add(
                &mut topology,
                document,
                module.clone(),
                format!("Module {name}"),
                Some(anchor),
            );
        }
        Ok(_) => {}
        Err(error) => document.facts.push(Fact::failure(
            "Associated module (optional)",
            root.join("module"),
            error,
        )),
    }
    bound(document, &mut topology, root, anchor, checkpoint);
    document.topology = Some(topology);
}

fn bound(
    document: &mut Document,
    topology: &mut Topology,
    driver: &Path,
    anchor: usize,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let entries = match std::fs::read_dir(driver) {
        Ok(entries) => entries,
        Err(error) => {
            document.facts.push(Fact::failure(
                "Bound device inventory",
                driver.into(),
                error,
            ));
            topology.coverage = Outcome::Unsupported;
            return;
        }
    };
    let (paths, coverage) = buses::select_children(
        entries,
        |entry| entry.file_type().is_ok_and(|kind| kind.is_symlink()),
        32,
    );
    if coverage != Outcome::Observed {
        topology.coverage = coverage.clone();
    }
    let mut count = 0;
    for path in paths {
        if let Some(target) = resolve(document, topology, &path, "Driver interface link") {
            if target.join("driver").canonicalize().ok().as_deref() == Some(driver) {
                let name = target.file_name().unwrap_or_default().to_string_lossy();
                buses::add(
                    topology,
                    document,
                    target.clone(),
                    format!("Bound device {name}"),
                    Some(anchor),
                );
                buses::access_node(document, &target, "Bound device access interface");
                count += 1;
            }
        }
        checkpoint(document);
    }
    let mut fact = Fact::text(
        "Bound device inventory",
        format!("{count} observed bindings; not proof of active use or exclusive ownership"),
        driver.into(),
    );
    fact.outcome = coverage;
    document.facts.push(fact);
    topology
        .summary
        .push_str(&format!(" · {count} observed bound devices"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn module_relationships_preserve_holder_direction_and_parameter_privacy() {
        let temp = tempfile::tempdir().unwrap();
        let modules = temp.path().join("sys/module");
        let core = modules.join("core");
        let client = modules.join("client");
        let driver = temp.path().join("sys/bus/pci/drivers/client");
        let device = temp.path().join("sys/devices/0000:01:00.0");
        for path in [
            core.join("holders"),
            client.join("parameters"),
            client.join("drivers"),
            driver.clone(),
            device.clone(),
        ] {
            std::fs::create_dir_all(path).unwrap();
        }
        std::fs::write(client.join("parameters/password"), b"must never be read").unwrap();
        symlink(&client, core.join("holders/client")).unwrap();
        symlink(&driver, client.join("drivers/pci:client")).unwrap();
        symlink(&client, driver.join("module")).unwrap();
        symlink(&driver, device.join("driver")).unwrap();
        symlink(&device, driver.join("0000:01:00.0")).unwrap();
        let mut doc = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        doc.facts.clear();
        doc.relations.clear();
        collect_at(&mut doc, &client, &modules, &mut |_| {});
        assert!(doc.facts.iter().any(
            |fact| fact.label == "Parameter password" && fact.outcome == Outcome::NotRequested
        ));
        assert!(doc.facts.iter().all(|fact| {
            !fact
                .value
                .as_deref()
                .is_some_and(|value| value.contains("must never be read"))
        }));
        let topology = doc.topology.as_ref().unwrap();
        assert!(
            topology
                .nodes
                .iter()
                .any(|node| node.path == core && node.label.starts_with("Dependency"))
        );
        assert!(
            topology
                .nodes
                .iter()
                .any(|node| node.path == device && node.label.starts_with("Bound device"))
        );
        assert!(
            doc.relations
                .iter()
                .any(|relation| relation.target == driver)
        );
        let context = super::super::attributes::Context::discover(&client.join("parameters"));
        assert!(super::super::attributes::rules(&context).is_empty());
        collect_at(&mut doc, &core, &modules, &mut |_| {});
        assert!(
            doc.topology
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .any(|node| node.path == client && node.label.starts_with("Dependent module"))
        );
    }

    #[test]
    fn driver_links_require_binding_backlinks_and_modules_are_optional() {
        let temp = tempfile::tempdir().unwrap();
        let modules = temp.path().join("sys/module");
        let root = temp.path().join("sys/bus/platform/drivers/builtin");
        let device = temp.path().join("sys/devices/example");
        let unrelated = temp.path().join("sys/devices/unrelated");
        for path in [&modules, &root, &device, &unrelated] {
            std::fs::create_dir_all(path).unwrap();
        }
        symlink(&root, device.join("driver")).unwrap();
        symlink(&device, root.join("example")).unwrap();
        symlink(&unrelated, root.join("unrelated")).unwrap();
        std::fs::write(root.join("bind"), b"not read").unwrap();
        std::fs::write(root.join("unbind"), b"not read").unwrap();
        let mut doc = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        doc.facts.clear();
        doc.relations.clear();
        collect_at(&mut doc, &root, &modules, &mut |_| {});
        assert_eq!(doc.title, "Kernel driver");
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "Associated module (optional)"
                    && fact.outcome == Outcome::Missing)
        );
        let topology = doc.topology.as_ref().unwrap();
        assert_eq!(topology.nodes.len(), 2);
        assert!(topology.nodes.iter().any(|node| node.path == device));
        assert!(
            !doc.relations.iter().any(
                |relation| relation.target == unrelated || relation.target == root.join("bind")
            )
        );
        assert!(topology.summary.contains("1 observed bound devices"));
        symlink(temp.path().join("gone"), root.join("gone-interface")).unwrap();
        collect_at(&mut doc, &root, &modules, &mut |_| {});
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "Driver interface link"
                    && fact.outcome == Outcome::Disappeared)
        );
    }

    #[test]
    fn parameter_inventory_limits_and_missing_fields_are_explicit() {
        let temp = tempfile::tempdir().unwrap();
        let modules = temp.path().join("module");
        let root = modules.join("example");
        std::fs::create_dir_all(root.join("parameters")).unwrap();
        for id in 0..40 {
            std::fs::write(root.join(format!("parameters/p{id:02}")), b"private").unwrap();
        }
        let mut doc = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        doc.facts.clear();
        collect_at(&mut doc, &root, &modules, &mut |_| {});
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "Module parameter inventory"
                    && fact.outcome == Outcome::Truncated)
        );
        assert_eq!(
            doc.facts
                .iter()
                .filter(|fact| fact.label.starts_with("Parameter "))
                .count(),
            32
        );
        assert_eq!(doc.topology.unwrap().coverage, Outcome::Truncated);
        assert_eq!(driver_bus(&root), None);
    }

    #[test]
    fn module_attributes_are_scoped_to_roots_not_arbitrary_subdirectories() {
        use super::super::attributes::{Context, Family, rules};
        let root = Context::from_parts(Path::new("/sys/module/example"), None);
        assert_eq!(root.family, Family::Module);
        assert!(rules(&root).iter().any(|rule| rule.name == "srcversion"));
        for name in ["parameters", "queue", "power"] {
            let path = Path::new("/sys/module").join(name);
            assert!(
                rules(&Context::from_parts(&path, None))
                    .iter()
                    .any(|rule| rule.name == "version")
            );
        }
        assert!(
            rules(&Context::from_parts(
                Path::new("/sys/bus/platform/drivers/power"),
                None
            ))
            .is_empty()
        );
        for name in ["nvme", "net", "cpu", "input"] {
            let path = Path::new("/sys/bus/pci/drivers").join(name);
            let context = Context::from_parts(&path, None);
            assert_eq!(context.family, Family::Driver);
            assert!(rules(&context).is_empty());
        }
        for directory in ["parameters", "sections", "notes", "holders", "drivers"] {
            let path = Path::new("/sys/module/example").join(directory);
            assert!(rules(&Context::from_parts(&path, None)).is_empty());
        }
    }
}
