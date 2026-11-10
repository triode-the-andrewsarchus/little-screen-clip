//! Identity-scoped Btrfs sysfs snapshots; no filesystem ioctls or device payloads.
use super::{
    Document, Relation,
    attributes::{Format, Rule},
    buses::{self, Topology},
    collector::Reads,
    model::{Fact, Outcome},
};
use std::{
    io,
    path::{Path, PathBuf},
};

const INSTANCE: &[(&str, &str, Format)] = &[
    ("label", "Filesystem label", Format::Text),
    ("metadata_uuid", "Metadata UUID", Format::Text),
    ("nodesize", "Tree node size", Format::Bytes),
    ("sectorsize", "Filesystem sector size", Format::Bytes),
    ("checksum", "Checksum", Format::Text),
    ("generation", "Filesystem generation", Format::Unsigned),
    ("exclusive_operation", "Exclusive operation", Format::Text),
    (
        "temp_fsid",
        "Temporary filesystem ID",
        Format::Boolean("Yes", "No"),
    ),
];
const SPACE: &[(&str, &str, Format)] = &[
    ("total_bytes", "Logical allocated", Format::Bytes),
    ("bytes_used", "Logical used", Format::Bytes),
    ("disk_total", "Device allocated", Format::Bytes),
    ("disk_used", "Device used", Format::Bytes),
];
const RESERVE: &[(&str, &str, Format)] = &[
    ("global_rsv_size", "Global reserve size", Format::Bytes),
    ("global_rsv_reserved", "Global reserve held", Format::Bytes),
];

pub(super) fn collect(
    document: &mut Document,
    root: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) -> bool {
    collect_at(
        document,
        root,
        Path::new("/sys/fs/btrfs"),
        reads,
        checkpoint,
    )
}
fn collect_at(
    document: &mut Document,
    root: &Path,
    reference: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) -> bool {
    let Some(base) = root
        .ancestors()
        .take(6)
        .find(|path| super::boot_firmware::same_object(path, reference))
    else {
        return false;
    };
    let Ok(relative) = root.strip_prefix(base) else {
        return false;
    };
    let names = relative
        .iter()
        .map(|name| name.to_str())
        .collect::<Option<Vec<_>>>();
    let Some(names) = names else {
        return false;
    };
    let requested = document.requested_attribute();
    match names.as_slice() {
        [] if requested.is_none() => inventory(document, root),
        ["features"] => {
            document.title = "Btrfs kernel features".into();
            features(document, root, "Supported kernel features");
        }
        [id, rest @ ..] if uuid::Uuid::parse_str(id).is_ok() => {
            if !instance(
                document,
                root,
                rest,
                requested.as_deref(),
                reads,
                checkpoint,
            ) {
                return false;
            }
            document.title = "Btrfs filesystem".into();
            document
                .facts
                .insert(0, Fact::text("Filesystem UUID", *id, base.join(id)));
        }
        _ => return false,
    }
    checkpoint(document);
    true
}
fn observe(
    document: &mut Document,
    root: &Path,
    specs: &[(&str, &str, Format)],
    requested: Option<&str>,
    prefix: &str,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) -> bool {
    if requested.is_some_and(|name| !specs.iter().any(|(field, _, _)| *field == name)) {
        return false;
    }
    for (name, label, format) in specs
        .iter()
        .filter(|(name, _, _)| requested.is_none_or(|value| *name == value))
    {
        let label = format!("{prefix}{label}");
        document
            .facts
            .push(reads.fact(root, &Rule::new(name, &label, *format)));
        checkpoint(document);
    }
    true
}
fn instance(
    document: &mut Document,
    root: &Path,
    scope: &[&str],
    requested: Option<&str>,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) -> bool {
    match scope {
        [] => {
            if !observe(document, root, INSTANCE, requested, "", reads, checkpoint) {
                return false;
            }
            if requested.is_none() {
                features(
                    document,
                    &root.join("features"),
                    "Enabled filesystem features",
                );
                observe(
                    document,
                    &root.join("allocation"),
                    RESERVE,
                    None,
                    "",
                    reads,
                    checkpoint,
                );
                let mut topology = Topology::new(
                    "Btrfs filesystem",
                    "Observed allocation and associated devices".into(),
                );
                let anchor = buses::add(
                    &mut topology,
                    document,
                    root.into(),
                    "Filesystem".into(),
                    None,
                );
                devices(document, &root.join("devices"), &mut topology, anchor);
                allocations(
                    document,
                    &root.join("allocation"),
                    reads,
                    &mut topology,
                    anchor,
                    checkpoint,
                );
                document.topology = Some(topology);
            }
        }
        ["features"] => features(document, root, "Enabled filesystem features"),
        ["allocation"] => {
            if !observe(document, root, RESERVE, requested, "", reads, checkpoint) {
                return false;
            }
            if requested.is_none() {
                let mut topology = Topology::new(
                    "Btrfs allocation",
                    "Logical and device allocation snapshots".into(),
                );
                let anchor = buses::add(
                    &mut topology,
                    document,
                    root.into(),
                    "Allocation".into(),
                    None,
                );
                allocations(document, root, reads, &mut topology, anchor, checkpoint);
                document.topology = Some(topology);
            }
        }
        ["allocation", class @ ("data" | "metadata" | "system")] => {
            return observe(
                document,
                root,
                SPACE,
                requested,
                &format!("{class} "),
                reads,
                checkpoint,
            );
        }
        ["devices"] if requested.is_none() => {
            let mut topology = Topology::new(
                "Btrfs devices",
                "Observed associated block-device objects".into(),
            );
            let anchor = buses::add(&mut topology, document, root.into(), "Devices".into(), None);
            devices(document, root, &mut topology, anchor);
            document.topology = Some(topology);
        }
        _ => return false,
    }
    document.facts.push(Fact::text("Filesystem interpretation",
        "Sysfs is not a complete filesystem or subvolume inventory. Logical allocation and device allocation differ; allocated space is not device capacity or user-visible free space. Fields are non-atomic snapshots. No filesystem operation or device payload read was issued.", root.into()));
    true
}
fn allocations(
    document: &mut Document,
    root: &Path,
    reads: &mut Reads,
    topology: &mut Topology,
    anchor: usize,
    checkpoint: &mut dyn FnMut(&Document),
) {
    for class in ["data", "metadata", "system"] {
        let directory = root.join(class);
        observe(
            document,
            &directory,
            SPACE,
            None,
            &format!("{class} "),
            reads,
            checkpoint,
        );
        let values = ["Logical used", "Logical allocated"].map(|label| {
            document
                .observed_raw(&format!("{class} {label}"))
                .and_then(|value| value.parse::<u64>().ok())
                .map(crate::file_manager::filesystem::format_size)
                .unwrap_or_else(|| "unavailable".into())
        });

        buses::add(
            topology,
            document,
            directory,
            format!("{class} · {}/{} logical", values[0], values[1]),
            Some(anchor),
        );
    }
}
fn entries(root: &Path, directories: bool) -> io::Result<(Vec<PathBuf>, Outcome)> {
    Ok(buses::select_children(
        std::fs::read_dir(root)?,
        |entry| {
            entry.file_type().is_ok_and(|kind| {
                if directories {
                    kind.is_dir() || kind.is_symlink()
                } else {
                    kind.is_file()
                }
            })
        },
        16,
    ))
}
fn features(document: &mut Document, root: &Path, label: &str) {
    match entries(root, false) {
        Ok((paths, coverage)) => {
            let names = paths
                .iter()
                .filter_map(|path| path.file_name())
                .map(|name| name.to_string_lossy())
                .collect::<Vec<_>>()
                .join(", ");
            let mut fact = Fact::text(label, names, root.into());
            fact.outcome = coverage;
            document.facts.push(fact);
        }
        Err(error) => document
            .facts
            .push(Fact::failure(label, root.into(), error)),
    }
}
fn device_target(path: &Path) -> io::Result<PathBuf> {
    let target = path.canonicalize()?;
    if buses::subsystem(&target).as_deref() != Some("block") {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Not a directly registered block-device object",
        ));
    }
    Ok(target)
}
fn devices(document: &mut Document, root: &Path, topology: &mut Topology, anchor: usize) {
    match entries(root, true) {
        Ok((paths, coverage)) => {
            topology.coverage = coverage;
            for path in paths {
                match device_target(&path) {
                    Ok(target) => {
                        let label = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();
                        document.relations.push(Relation::new(
                            format!("Filesystem block device {label}"),
                            target.clone(),
                        ));
                        buses::add(topology, document, target, label, Some(anchor));
                    }
                    Err(error) => {
                        let fact = Fact::failure("Filesystem device association", path, error);
                        topology.coverage = fact.outcome.clone();
                        document.facts.push(fact);
                    }
                }
            }
        }
        Err(error) => {
            let fact = Fact::failure("Filesystem devices", root.into(), error);
            topology.coverage = fact.outcome.clone();
            document.facts.push(fact);
        }
    }
}
fn inventory(document: &mut Document, root: &Path) {
    document.title = "Btrfs filesystem instances".into();
    let mut topology = Topology::new(
        "Btrfs instances",
        "Observed sysfs filesystem instances, not subvolumes".into(),
    );
    match entries(root, true) {
        Ok((paths, coverage)) => {
            topology.coverage = coverage;
            for path in paths.into_iter().filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| uuid::Uuid::parse_str(name).is_ok())
            }) {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                buses::add(&mut topology, document, path, name, None);
            }
        }
        Err(error) => {
            document
                .facts
                .push(Fact::failure("Filesystem instances", root.into(), error))
        }
    }
    document.topology = Some(topology);
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::kernel_interface::{Filesystem, Kind};
    use std::collections::BTreeMap;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, Reads, Document) {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().join("btrfs");
        let root = base.join("00000000-0000-0000-0000-000000000001");
        std::fs::create_dir_all(root.join("features")).unwrap();
        std::fs::write(root.join("features/compress_zstd"), [0xff, 0xfe]).unwrap();
        std::fs::create_dir(root.join("devices")).unwrap();
        let device = temp.path().join("block-device");
        std::fs::create_dir(&device).unwrap();
        let class = temp.path().join("block");
        std::fs::create_dir(&class).unwrap();
        std::os::unix::fs::symlink(class, device.join("subsystem")).unwrap();
        std::os::unix::fs::symlink(device, root.join("devices/disk0")).unwrap();
        let mut values = BTreeMap::from([
            (root.join("label"), b"Example filesystem".to_vec()),
            (root.join("sectorsize"), b"4096".to_vec()),
            (root.join("exclusive_operation"), b"none".to_vec()),
        ]);
        for class in ["data", "metadata", "system"] {
            let directory = root.join("allocation").join(class);
            std::fs::create_dir_all(&directory).unwrap();
            for (name, raw) in [
                ("total_bytes", "1024"),
                ("bytes_used", "256"),
                ("disk_total", "2048"),
                ("disk_used", "512"),
            ] {
                values.insert(directory.join(name), raw.as_bytes().to_vec());
            }
        }
        let reads = Reads::fixture(values);
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        document.path = root.clone();
        document.interface_directory = true;
        document.kind = Kind::Kernel {
            filesystem: Filesystem::Sysfs,
        };
        (temp, base, root, reads, document)
    }

    #[test]
    fn mounted_instance_shows_distinct_allocation_spaces_features_and_devices() {
        let (_temp, base, root, mut reads, mut document) = fixture();
        assert!(collect_at(
            &mut document,
            &root,
            &base,
            &mut reads,
            &mut |_| {}
        ));
        assert_eq!(
            document.observed_raw("Filesystem UUID"),
            Some("00000000-0000-0000-0000-000000000001")
        );
        assert_eq!(document.observed_raw("data Logical used"), Some("256"));
        assert_eq!(document.observed_raw("data Device used"), Some("512"));
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "Enabled filesystem features"
                    && fact.value.as_deref() == Some("compress_zstd"))
        );
        assert!(
            reads
                .attempts()
                .all(|path| !path.starts_with(root.join("features"))
                    && !path.starts_with(root.join("devices")))
        );
        assert_eq!(document.topology.as_ref().unwrap().nodes.len(), 5);
        assert!(
            document.topology.as_ref().unwrap().nodes[2]
                .label
                .contains("1.0 KiB")
        );
        assert!(
            document
                .relations
                .iter()
                .any(|relation| relation.label == "Filesystem block device disk0")
        );
    }

    #[test]
    fn attribute_scope_reads_only_requested_allocation_field_and_rejects_controls() {
        let (_temp, base, root, mut reads, mut document) = fixture();
        let directory = root.join("allocation/data");
        document.path = directory.join("bytes_used");
        document.interface_directory = false;
        assert!(collect_at(
            &mut document,
            &directory,
            &base,
            &mut reads,
            &mut |_| {}
        ));
        assert_eq!(reads.attempts().count(), 1);
        assert_eq!(document.observed_raw("data Logical used"), Some("256"));
        for control in [
            "force_chunk_alloc",
            "bg_reclaim_threshold",
            "unknown_binary",
        ] {
            document.path = directory.join(control);
            assert!(!collect_at(
                &mut document,
                &directory,
                &base,
                &mut reads,
                &mut |_| {}
            ));
        }
        assert_eq!(reads.attempts().count(), 1);
    }

    #[test]
    fn unverified_device_links_disclose_partial_inventory_instead_of_claiming_block_membership() {
        let (temp, base, root, mut reads, mut document) = fixture();
        std::fs::remove_file(temp.path().join("block-device/subsystem")).unwrap();
        assert!(collect_at(
            &mut document,
            &root,
            &base,
            &mut reads,
            &mut |_| {}
        ));
        assert_eq!(
            document.topology.as_ref().unwrap().coverage,
            Outcome::Unsupported
        );
        assert_eq!(document.topology.as_ref().unwrap().nodes.len(), 4);
        assert!(
            !document
                .relations
                .iter()
                .any(|relation| relation.label == "Filesystem block device disk0")
        );
    }

    #[test]
    fn lookalike_root_cannot_authorize_reads_and_feature_inventory_is_bounded() {
        let (temp, base, root, mut reads, mut document) = fixture();
        let impostor = temp.path().join("impostor");
        std::fs::create_dir(&impostor).unwrap();
        assert!(!collect_at(
            &mut document,
            &root,
            &impostor,
            &mut reads,
            &mut |_| {}
        ));
        assert_eq!(reads.attempts().count(), 0);
        for index in 0..20 {
            std::fs::write(
                root.join("features").join(format!("flag{index}")),
                b"ignored",
            )
            .unwrap();
        }
        assert!(collect_at(
            &mut document,
            &root,
            &base,
            &mut reads,
            &mut |_| {}
        ));
        let features = document
            .facts
            .iter()
            .find(|fact| fact.label == "Enabled filesystem features")
            .unwrap();
        assert_eq!(features.outcome, Outcome::Truncated);
    }
}
