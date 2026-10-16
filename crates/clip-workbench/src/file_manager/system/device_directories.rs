//! Bounded device-namespace inventories; access nodes are pinned only for metadata.
mod registered;
use super::{
    Document, Relation,
    buses::{self, Topology},
    identity::ObjectContext,
    model::{Fact, Outcome, Value},
};
use little_clip_core::kernel_interface::{Filesystem, Kind};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

const ENTRY_LIMIT: usize = 128;

#[derive(Clone, Copy)]
enum Role {
    Terminals,
    Aliases(usize),
    Registered(registered::Family),
}

fn role(document: &Document) -> Option<Role> {
    if !document.interface_directory {
        return None;
    }
    if document.kind
        == (Kind::Kernel {
            filesystem: Filesystem::Devpts,
        })
    {
        return Some(Role::Terminals);
    }
    for (path, family) in [
        ("/dev/dri", registered::Family::Display),
        ("/dev/snd", registered::Family::Sound),
        ("/dev/input", registered::Family::Input),
        ("/dev/cpu", registered::Family::Cpu),
        ("/dev/bus/usb", registered::Family::Usb),
        ("/dev/vfio", registered::Family::Vfio),
        ("/dev/mapper", registered::Family::Mapper),
    ] {
        if super::boot_firmware::same_object(&document.path, Path::new(path)) {
            return Some(Role::Registered(family));
        }
    }
    for root in ["/dev/disk", "/dev/serial"] {
        if super::boot_firmware::same_object(&document.path, Path::new(root)) {
            return Some(Role::Aliases(1));
        }
        for name in [
            "by-id",
            "by-uuid",
            "by-label",
            "by-path",
            "by-partuuid",
            "by-partlabel",
        ] {
            if super::boot_firmware::same_object(&document.path, &Path::new(root).join(name)) {
                return Some(Role::Aliases(0));
            }
        }
    }
    None
}

pub(super) fn collect(document: &mut Document) {
    let Some(role) = role(document) else { return };
    collect_role(document, role);
}

/// Never follows directory symlinks; the total entry budget spans all levels.
fn inventory(document: &mut Document, depth: usize) -> (Vec<PathBuf>, Outcome) {
    let mut pending = vec![(document.path.clone(), depth)];
    let mut paths = Vec::new();
    let mut visited = 0;
    let mut coverage = Outcome::Observed;
    'scan: while let Some((root, depth)) = pending.pop() {
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) => {
                let fact = Fact::failure("Directory inventory", root, error);
                coverage = fact.outcome.clone();
                document.facts.push(fact);
                continue;
            }
        };
        for entry in entries {
            if visited == ENTRY_LIMIT {
                coverage = Outcome::Truncated;
                break 'scan;
            }
            visited += 1;
            match entry {
                Ok(entry) => match entry.file_type() {
                    Ok(kind) if kind.is_dir() && depth > 0 => {
                        paths.push(entry.path());
                        pending.push((entry.path(), depth - 1));
                    }
                    Ok(_) => paths.push(entry.path()),
                    Err(error) => {
                        coverage = Outcome::Unsupported;
                        document
                            .facts
                            .push(Fact::failure("Directory entry", entry.path(), error));
                    }
                },
                Err(error) => {
                    coverage = Outcome::Unsupported;
                    document
                        .facts
                        .push(Fact::failure("Directory entry", root.clone(), error));
                }
            }
        }
    }
    paths.sort();
    (paths, coverage)
}

fn collect_role(document: &mut Document, role: Role) {
    if let Role::Registered(family) = role {
        registered::collect(document, family);
        return;
    }
    let (heading, depth) = match role {
        Role::Terminals => ("Allocated pseudo-terminals", 0),
        Role::Aliases(depth) => ("Stable device aliases", depth),
        Role::Registered(_) => unreachable!(),
    };
    let (paths, coverage) = inventory(document, depth);
    let mut topology = Topology::new(heading, String::new());
    topology.coverage = coverage;
    let mut groups = BTreeMap::new();
    for path in paths {
        match role {
            Role::Terminals => terminal(document, &mut topology, path),
            Role::Aliases(_) => alias(document, &mut topology, &mut groups, path),
            Role::Registered(_) => unreachable!(),
        }
    }
    topology.summary = match role {
        Role::Terminals => format!("{} observed allocated PTYs", topology.nodes.len()),
        Role::Aliases(_) => format!(
            "{} observed aliases · {} distinct resolved device targets",
            topology.nodes.len().saturating_sub(groups.len()),
            groups.len()
        ),
        Role::Registered(_) => unreachable!(),
    };
    let count = matches!(role, Role::Terminals).then_some(topology.nodes.len());
    finish(document, topology, heading, count);
}

fn finish(document: &mut Document, mut topology: Topology, heading: &str, count: Option<usize>) {
    if topology.coverage != Outcome::Observed {
        topology
            .summary
            .push_str(&format!(" · {}", topology.coverage));
    }
    let mut fact = Fact::text(heading, topology.summary.clone(), document.path.clone());
    fact.outcome = topology.coverage.clone();
    if let Some(count) = count {
        fact.semantic = Some(Value::Integer(count as i128));
    }
    document.facts.insert(0, fact);
    document.title = heading.into();
    document.topology = Some(topology);
}

fn terminal(document: &mut Document, topology: &mut Topology, path: PathBuf) {
    let Some(number) = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|name| name.parse::<u32>().ok())
    else {
        return;
    };
    let child = match super::worker::interface(&path) {
        Ok(child) => child,
        Err(error) => {
            unavailable(document, topology, path, io::Error::other(error));
            return;
        }
    };
    let same_mount = document
        .interface_context
        .zip(child.interface_context)
        .is_some_and(|(parent, child)| parent.object.same_hierarchy_mount(child.object));
    if same_mount
        && matches!(
            child.kind,
            Kind::Character {
                major: 136..=143,
                ..
            }
        )
    {
        buses::add_relation(
            topology,
            document,
            Relation::observed_interface(format!("PTY {number}"), path, &child),
            None,
        );
    } else {
        unavailable(
            document,
            topology,
            path,
            io::Error::new(
                io::ErrorKind::Unsupported,
                "Not a slave PTY in the selected devpts mount",
            ),
        );
    }
}

fn unavailable(document: &mut Document, topology: &mut Topology, path: PathBuf, error: io::Error) {
    let fact = Fact::failure("Directory target", path, error);
    if topology.coverage == Outcome::Observed {
        topology.coverage = fact.outcome.clone();
    }
    document.facts.push(fact);
}

fn alias(
    document: &mut Document,
    topology: &mut Topology,
    groups: &mut BTreeMap<ObjectContext, usize>,
    path: PathBuf,
) {
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {}
        Ok(_) => return,
        Err(error) => {
            unavailable(document, topology, path, error);
            return;
        }
    }
    if let Err(error) = std::fs::metadata(&path) {
        unavailable(document, topology, path, error);
        return;
    }
    let child = match super::worker::interface(&path) {
        Ok(child) if matches!(child.kind, Kind::Character { .. } | Kind::Block { .. }) => child,
        result => {
            unavailable(
                document,
                topology,
                path,
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!(
                        "Device alias target unavailable: {}",
                        result
                            .err()
                            .unwrap_or_else(|| "not a device interface".into())
                    ),
                ),
            );
            return;
        }
    };
    let Some(context) = child.interface_context else {
        unavailable(
            document,
            topology,
            path,
            io::Error::new(
                io::ErrorKind::Unsupported,
                "Device target mount identity unavailable",
            ),
        );
        return;
    };
    let parent = *groups.entry(context).or_insert_with(|| {
        buses::add_relation(
            topology,
            document,
            Relation::observed_interface(
                super::super::display_path(&child.path),
                child.path.clone(),
                &child,
            ),
            None,
        )
    });
    let label = format!(
        "{} → {}",
        super::super::display_path(path.strip_prefix(&document.path).unwrap_or(&path)),
        super::super::display_path(&child.path)
    );
    buses::add_relation(
        topology,
        document,
        Relation::observed_interface(label, path, &child),
        Some(parent),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    pub(super) fn document(path: &Path) -> Document {
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        let pinned = little_clip_core::kernel_interface::pin(path, true).unwrap();
        document.interface_context = ObjectContext::pinned(&pinned).ok();
        document.path = path.into();
        document.kind = little_clip_core::kernel_interface::classify_handle(&pinned).unwrap();
        document.interface_directory = true;
        document.relations.clear();
        document.facts.clear();
        document
    }

    #[test]
    fn stable_aliases_share_targets_and_keep_distinct_interfaces_and_alias_links() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("by-id")).unwrap();
        for (name, target) in [
            ("first", "/dev/null"),
            ("same", "/dev/null"),
            ("other", "/dev/zero"),
        ] {
            symlink(target, temp.path().join("by-id").join(name)).unwrap();
        }
        symlink(temp.path(), temp.path().join("loop")).unwrap();
        let mut doc = document(temp.path());
        collect_role(&mut doc, Role::Aliases(1));
        let topology = doc.topology.unwrap();
        assert_eq!(topology.coverage, Outcome::Unsupported);
        assert_eq!(topology.nodes.len(), 5);
        assert_eq!(
            topology
                .nodes
                .iter()
                .filter(|node| node.parent.is_none())
                .count(),
            2
        );
        let first = topology
            .nodes
            .iter()
            .find(|node| node.label.starts_with("by-id/first"))
            .unwrap();
        let same = topology
            .nodes
            .iter()
            .find(|node| node.label.starts_with("by-id/same"))
            .unwrap();
        assert_eq!(first.parent, same.parent);
        assert!(topology.summary.contains("3 observed aliases"));
        assert_eq!(doc.relations.len(), 5);
    }

    #[test]
    fn broken_non_device_and_truncated_alias_inventories_never_claim_successful_empty() {
        let temp = tempfile::tempdir().unwrap();
        symlink(temp.path().join("missing"), temp.path().join("broken")).unwrap();
        let mut doc = document(temp.path());
        collect_role(&mut doc, Role::Aliases(0));
        assert_eq!(doc.topology.as_ref().unwrap().coverage, Outcome::Missing);
        assert_eq!(
            doc.facts
                .iter()
                .find(|fact| fact.label == "Directory target")
                .unwrap()
                .outcome,
            Outcome::Missing
        );
        std::fs::remove_file(temp.path().join("broken")).unwrap();
        std::fs::write(
            temp.path().join("ordinary"),
            b"Never preview alias payloads here",
        )
        .unwrap();
        symlink(temp.path().join("ordinary"), temp.path().join("not-device")).unwrap();
        let mut doc = document(temp.path());
        collect_role(&mut doc, Role::Aliases(0));
        assert_eq!(
            doc.topology.as_ref().unwrap().coverage,
            Outcome::Unsupported
        );
        assert!(doc.topology.as_ref().unwrap().nodes.is_empty());
        std::fs::remove_file(temp.path().join("not-device")).unwrap();
        std::fs::remove_file(temp.path().join("ordinary")).unwrap();
        for index in 0..140 {
            symlink("/dev/null", temp.path().join(format!("alias{index}"))).unwrap();
        }
        let mut doc = document(temp.path());
        collect_role(&mut doc, Role::Aliases(0));
        assert_eq!(doc.topology.as_ref().unwrap().coverage, Outcome::Truncated);
        assert_eq!(doc.topology.as_ref().unwrap().nodes.len(), ENTRY_LIMIT + 1);
    }

    #[test]
    fn pty_directory_counts_owned_slave_and_excludes_allocator() {
        use std::os::fd::{AsRawFd, FromRawFd};
        // Allocate only this fixture's PTY pair; never open an existing terminal.
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: output descriptors point to initialized writable integers;
        // null optional arguments request default termios/window size and no name.
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        // SAFETY: successful openpty returned two new descriptors, each adopted once.
        let _master = unsafe { std::fs::File::from_raw_fd(master) };
        // SAFETY: slave is the other newly owned descriptor from openpty.
        let slave = unsafe { std::fs::File::from_raw_fd(slave) };
        let target = std::fs::read_link(format!("/proc/self/fd/{}", slave.as_raw_fd())).unwrap();
        let root = target.parent().unwrap();
        let mut doc = document(root);
        collect(&mut doc);
        let topology = doc.topology.unwrap();
        assert!(topology.nodes.iter().any(|node| node.path == target));
        assert!(
            !topology
                .nodes
                .iter()
                .any(|node| node.path.file_name().unwrap() == "ptmx")
        );
        assert!(topology.nodes.iter().all(|node| node.parent.is_none()));
        assert_eq!(
            doc.facts[0].semantic,
            Some(Value::Integer(topology.nodes.len() as i128))
        );
    }
}
