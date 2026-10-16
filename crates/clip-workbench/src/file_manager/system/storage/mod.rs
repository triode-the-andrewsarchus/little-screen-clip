//! Bounded block topology from sysfs, udev metadata and this mount namespace.
pub(super) mod loops;
pub(super) mod mounts;
mod raid;
mod ui;
use super::{
    Document, Relation,
    attributes::{Context, Family},
    collector::Reads,
    model::{Fact, Outcome},
};
use mounts::{Mount, device_number};
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
};
pub(super) use ui::{inspector, paint};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::file_manager) struct Partition {
    pub name: String,
    #[serde(with = "super::path_serde")]
    pub path: PathBuf,
    pub start: u64,
    pub bytes: u64,
    pub selected: bool,
    pub filesystem: Option<String>,
    pub label: Option<String>,
    pub uuid: Option<String>,
    #[serde(with = "super::path_serde::list")]
    pub mounts: Vec<PathBuf>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::file_manager) enum Layout {
    #[default]
    Block,
    Flash,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::file_manager) struct Storage {
    #[serde(default)]
    pub layout: Layout,
    pub name: String,
    pub class: String,
    pub capacity: Option<u64>,
    #[serde(with = "super::path_serde")]
    pub disk: PathBuf,
    pub disk_capacity: Option<u64>,
    pub partitions: Vec<Partition>,
    pub coverage: Outcome,
    pub mounts: Vec<Mount>,
    #[serde(with = "super::path_serde::list")]
    pub dependencies: Vec<PathBuf>,
    #[serde(with = "super::path_serde::list")]
    pub holders: Vec<PathBuf>,
    #[serde(with = "super::path_serde::optional")]
    pub backing: Option<PathBuf>,
    pub retained: bool,
}
impl Storage {
    pub fn summary(&self) -> String {
        let capacity = self
            .capacity
            .map(bytes)
            .unwrap_or_else(|| "capacity unavailable".into());
        let mounted = self
            .mounts
            .iter()
            .take(2)
            .map(|mount| format!("{} at {}", mount.filesystem, mount.target.display()))
            .collect::<Vec<_>>()
            .join(" · ");
        let backing = self
            .backing
            .as_ref()
            .map(|path| format!(" · {}", path.display()))
            .unwrap_or_default();
        format!(
            "{} · {capacity}{backing}{}",
            self.class,
            if mounted.is_empty() {
                String::new()
            } else {
                format!(" · {mounted}")
            }
        )
    }
}
use crate::file_manager::filesystem::format_size as bytes;
fn capacity(reads: &mut Reads, path: &Path) -> io::Result<u64> {
    reads
        .unsigned(path)?
        .checked_mul(512)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Block capacity overflow"))
}
fn class(document: &Document, sys: &Path, size: Option<u64>) -> String {
    let name = sys.file_name().unwrap_or_default().to_string_lossy();
    if document.observed_raw("Partition number").is_some() {
        return "Partition".into();
    }
    if name
        .strip_prefix("loop")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()))
    {
        return if size == Some(0)
            && document
                .facts
                .iter()
                .any(|fact| fact.label == "Backing file" && fact.outcome == Outcome::Missing)
        {
            "Unattached loop device".into()
        } else {
            "Loop device".into()
        };
    }
    if document.observed_raw("Mapped name").is_some() {
        return "Device-mapper volume".into();
    }
    if sys.join("md").is_dir() {
        return "MD RAID array".into();
    }
    let virtual_path = sys.iter().any(|component| component == "virtual");
    if name.starts_with("nvme") {
        return if virtual_path {
            "Virtual NVMe namespace".into()
        } else {
            "NVMe namespace".into()
        };
    }
    let driver = sys.join("device/driver").canonicalize().ok();
    if driver
        .as_ref()
        .is_some_and(|path| path.file_name().is_some_and(|name| name == "virtio_blk"))
    {
        return "VirtIO virtual disk".into();
    }
    if virtual_path {
        "Virtual block device".into()
    } else {
        "Block disk".into()
    }
}
pub(super) fn collect(
    document: &mut Document,
    sys: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    // Queue/loop/dm subdirectories and generic class containers are not disks.
    let Some(number) = reads
        .read(&sys.join("dev"))
        .ok()
        .and_then(|value| device_number(&value))
    else {
        return;
    };
    let size = capacity(reads, &sys.join("size")).ok();
    let partition = document.observed_raw("Partition number").is_some();
    let disk = if partition {
        sys.parent().unwrap_or(sys)
    } else {
        sys
    };
    let mut storage = Storage {
        layout: Layout::Block,
        name: sys.file_name().unwrap_or_default().to_string_lossy().into(),
        class: class(document, sys, size),
        capacity: size,
        disk: disk.into(),
        disk_capacity: capacity(reads, &disk.join("size")).ok(),
        partitions: Vec::new(),
        coverage: Outcome::Observed,
        mounts: Vec::new(),
        dependencies: Vec::new(),
        holders: Vec::new(),
        backing: None,
        retained: false,
    };
    if partition {
        document
            .relations
            .push(Relation::new("Parent disk", disk.into()));
    }
    backing(document, &mut storage, sys, reads);
    dependencies(document, sys, &mut storage);
    filesystem_metadata(document, sys);
    let mounts = mount_associations(document, number, reads, &mut storage);
    partitions(
        document,
        disk,
        sys,
        reads,
        &mut storage,
        &mounts,
        checkpoint,
    );
    document.title = format!("{} · {}", storage.class, storage.name);
    document.storage = Some(storage);
    raid::collect(document, sys, reads);
    checkpoint(document);
}
fn backing(document: &mut Document, storage: &mut Storage, sys: &Path, reads: &mut Reads) {
    if let Ok(path) = reads.loop_backing(sys) {
        if let Some(fact) = document
            .facts
            .iter_mut()
            .find(|fact| fact.label == "Backing file")
        {
            let display = path.to_string_lossy().into_owned();
            fact.value = Some(display.clone());
            fact.raw = path.to_str().map(str::to_owned);
            fact.semantic = Some(super::model::Value::Text(display));
            fact.outcome = Outcome::Observed;
            fact.error = None;
        }
        let mut relation = Relation::new("Backing file", path.clone());
        match loops::Association::capture(sys, &path, reads) {
            Ok(association) => relation.loop_association = Some(association),
            Err(error) => {
                relation.identity = None;
                let mut fact = Fact::failure(
                    "Backing link verification",
                    sys.join("loop/backing_file"),
                    error,
                );
                fact.value = Some("Attachment not verified; refresh before navigating".into());
                document.facts.push(fact);
            }
        }
        document.relations.push(relation);
        storage.backing = Some(path);
    }
    if let Some(uuid) = document.observed_raw("Mapped UUID") {
        let marker = if uuid.starts_with("CRYPT-LUKS1-") {
            Some("LUKS1")
        } else if uuid.starts_with("CRYPT-LUKS2-") {
            Some("LUKS2")
        } else if uuid.starts_with("LVM-") {
            Some("LVM")
        } else {
            None
        };
        if let Some(marker) = marker {
            document.facts.push(Fact::text(
                "Mapped identity marker",
                format!("{marker} (from UUID; target type not queried)"),
                storage.disk.join("dm/uuid"),
            ));
        }
    }
}
pub(super) fn directory_group(
    sys: &Path,
    kind: little_clip_core::kernel_interface::Kind,
) -> Option<(Option<PathBuf>, String)> {
    use little_clip_core::kernel_interface::Kind;
    let name = sys.file_name()?.to_str()?;
    match kind {
        Kind::Block { .. }
            if Context::discover(sys).family == Family::Block
                && super::buses::numbered(name, "dm-") =>
        {
            Some((Some(sys.into()), "Device-mapper volume".into()))
        }
        Kind::Character { .. }
            if super::buses::subsystem(sys).as_deref() == Some("misc")
                && name == "device-mapper" =>
        {
            Some((None, "Device-mapper control interface (unopened)".into()))
        }
        _ => None,
    }
}

pub(super) fn directory_dependencies(
    document: &mut Document,
    sys: &Path,
    topology: &mut super::buses::Topology,
    parent: usize,
) {
    let first = document.facts.len();
    for target in dependency_group(document, sys, "slaves", "Backing block device") {
        topology.nodes.push(super::buses::Node {
            label: format!(
                "Backing block device · {}",
                target.file_name().unwrap_or_default().to_string_lossy()
            ),
            path: target,
            parent: Some(parent),
        });
    }
    if topology.coverage == super::model::Outcome::Observed
        && let Some(fact) = document.facts[first..]
            .iter()
            .find(|fact| fact.outcome != super::model::Outcome::Observed)
    {
        topology.coverage = fact.outcome.clone();
    }
}

fn dependency_group(document: &mut Document, sys: &Path, name: &str, label: &str) -> Vec<PathBuf> {
    let directory = sys.join(name);
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) => {
            document.facts.push(Fact::failure(
                format!("{label} inventory"),
                directory,
                error,
            ));
            return Vec::new();
        }
    };
    let mut targets = Vec::new();
    for (index, entry) in entries.enumerate().take(33) {
        if index == 32 {
            document.facts.push(Fact::failure(
                format!("{label} inventory"),
                directory.clone(),
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Dependency inventory capped at 32 links",
                ),
            ));
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                document.facts.push(Fact::failure(
                    format!("{label} inventory"),
                    directory.clone(),
                    error,
                ));
                continue;
            }
        };
        let target = match entry.path().canonicalize() {
            Ok(target) => target,
            Err(error) => {
                document.facts.push(Fact::failure(
                    format!("{label} relationship"),
                    entry.path(),
                    error,
                ));
                continue;
            }
        };
        if Context::discover(&target).family != Family::Block {
            document.facts.push(Fact::failure(
                format!("{label} relationship"),
                target,
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Target is not a registered block object",
                ),
            ));
            continue;
        }
        document
            .relations
            .push(Relation::new(label, target.clone()));
        targets.push(target);
    }
    targets.sort();
    targets
}
fn dependencies(document: &mut Document, sys: &Path, storage: &mut Storage) {
    storage.dependencies = dependency_group(document, sys, "slaves", "Backing block device");
    storage.holders = dependency_group(document, sys, "holders", "Holder");
}
fn filesystem_metadata(document: &mut Document, sys: &Path) {
    let Ok(device) = udev::Device::from_syspath(sys) else {
        return;
    };
    for (property, label) in [
        ("ID_FS_TYPE", "Filesystem (device database)"),
        ("ID_FS_LABEL", "Filesystem label"),
        ("ID_FS_UUID", "Filesystem UUID"),
        ("ID_PART_ENTRY_TYPE", "Partition type"),
        ("ID_PART_ENTRY_UUID", "Partition UUID"),
        ("ID_PART_ENTRY_NAME", "Partition label"),
    ] {
        if let Some(fact) = super::buses::database_fact(&device, sys, property, label) {
            document.facts.push(fact);
        }
    }
}
fn mount_associations(
    document: &mut Document,
    number: (u32, u32),
    reads: &mut Reads,
    storage: &mut Storage,
) -> Vec<Mount> {
    let source = PathBuf::from("/proc/self/mountinfo");
    let inventory = match reads.mountinfo() {
        Ok(bytes) => mounts::parse(&bytes),
        Err(error) => {
            document
                .facts
                .push(Fact::failure("Mount inventory", source, error));
            return Vec::new();
        }
    };
    let mut sources = std::collections::BTreeMap::new();
    let mut coverage = inventory.outcome;
    for mount in &inventory.records {
        if !mount_matches(mount, number, &mut sources, &mut coverage) {
            continue;
        }
        if storage.mounts.len() >= 128 {
            coverage = Outcome::Truncated;
            break;
        }
        document.relations.push(Relation::new(
            format!("{} mount", mount.filesystem),
            mount.target.clone(),
        ));
        storage.mounts.push(mount.clone());
    }
    let mut fact = Fact::text(
        "Mount inventory",
        format!(
            "{} associated mount(s) observed in this namespace; coverage: {}; {} malformed record(s)",
            storage.mounts.len(),
            coverage,
            inventory.malformed
        ),
        source.clone(),
    );
    fact.outcome = coverage;
    fact.origin = "mount namespace table".into();
    document.facts.push(fact);
    for mount in &storage.mounts {
        let mut fact = Fact::text(
            "Mounted filesystem",
            format!(
                "{} · {} · root {} · {}",
                mount.filesystem,
                mount.target.display(),
                mount.root.display(),
                if mount.read_only {
                    "read-only mount"
                } else {
                    "read-write mount flag"
                }
            ),
            source.clone(),
        );
        fact.origin = "mount namespace table".into();
        document.facts.push(fact);
    }
    inventory.records
}

fn mount_matches(
    mount: &Mount,
    number: (u32, u32),
    sources: &mut std::collections::BTreeMap<PathBuf, Option<(u32, u32)>>,
    coverage: &mut Outcome,
) -> bool {
    if mount.device == number {
        return true;
    }
    if mount.filesystem != "btrfs" || !mount.source.is_absolute() {
        return false;
    }
    if let Some(device) = sources.get(&mount.source) {
        return *device == Some(number);
    }
    if sources.len() >= 128 {
        *coverage = Outcome::Truncated;
        return false;
    }
    let device = match little_clip_core::kernel_interface::classify(&mount.source) {
        Ok(little_clip_core::kernel_interface::Kind::Block { major, minor }) => {
            Some((major, minor))
        }
        _ => {
            if *coverage == Outcome::Observed {
                *coverage = Outcome::Unsupported;
            }
            None
        }
    };
    sources.insert(mount.source.clone(), device);
    device == Some(number)
}
fn partition(
    reads: &mut Reads,
    path: &Path,
    selected: &Path,
    disk_bytes: Option<u64>,
) -> io::Result<Partition> {
    reads.unsigned(&path.join("partition"))?;
    let start = capacity(reads, &path.join("start"))?;
    let bytes = capacity(reads, &path.join("size"))?;
    let end = start
        .checked_add(bytes)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Partition extent overflow"))?;
    if disk_bytes.is_some_and(|capacity| end > capacity) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Partition lies outside the observed disk capacity",
        ));
    }
    Ok(Partition {
        name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into(),
        path: path.into(),
        start,
        bytes,
        selected: path == selected,
        filesystem: None,
        label: None,
        uuid: None,
        mounts: Vec::new(),
    })
}
fn partition_metadata(partition: &mut Partition, reads: &mut Reads, mounts: &[Mount]) {
    if let Ok(device) = udev::Device::from_syspath(&partition.path) {
        let property = |name: &str| {
            device
                .property_value(name)
                .and_then(|value| value.to_str())
                .map(|value| value.chars().take(256).collect::<String>())
        };
        partition.filesystem = property("ID_FS_TYPE");
        partition.label = property("ID_FS_LABEL");
        partition.uuid = property("ID_FS_UUID");
    }
    let Some(number) = reads
        .read(&partition.path.join("dev"))
        .ok()
        .and_then(|value| device_number(&value))
    else {
        return;
    };
    let mut sources = std::collections::BTreeMap::new();
    let mut coverage = Outcome::Observed;
    partition.mounts = mounts
        .iter()
        .filter(|mount| mount_matches(mount, number, &mut sources, &mut coverage))
        .take(8)
        .map(|mount| mount.target.clone())
        .collect();
}
fn inventory_failure(
    document: &mut Document,
    coverage: &mut Outcome,
    label: &str,
    path: PathBuf,
    error: io::Error,
) {
    let fact = Fact::failure(label, path, error);
    if *coverage == Outcome::Observed {
        *coverage = fact.outcome.clone();
    }
    document.facts.push(fact);
}

fn partitions(
    document: &mut Document,
    disk: &Path,
    selected: &Path,
    reads: &mut Reads,
    storage: &mut Storage,
    mounts: &[Mount],
    checkpoint: &mut dyn FnMut(&Document),
) {
    let entries = match std::fs::read_dir(disk) {
        Ok(entries) => entries,
        Err(error) => {
            inventory_failure(
                document,
                &mut storage.coverage,
                "Partition inventory",
                disk.into(),
                error,
            );
            return;
        }
    };
    let mut candidates = Vec::new();
    for (index, entry) in entries.enumerate().take(513) {
        if index == 512 {
            storage.coverage = Outcome::Truncated;
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                inventory_failure(
                    document,
                    &mut storage.coverage,
                    "Partition inventory entry",
                    disk.into(),
                    error,
                );
                continue;
            }
        };
        let marker = entry.path().join("partition");
        match std::fs::metadata(&marker) {
            Ok(metadata) if metadata.is_file() => candidates.push(entry.path()),
            Err(error)
                if !matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                inventory_failure(
                    document,
                    &mut storage.coverage,
                    "Partition membership",
                    marker,
                    error,
                )
            }
            _ => {}
        }
    }
    candidates.sort();
    if candidates.len() > 12 {
        storage.coverage = Outcome::Truncated;
        candidates.truncate(12);
    }
    let mut completed_coverage = storage.coverage.clone();
    storage.coverage = Outcome::Truncated;
    for path in candidates {
        match partition(reads, &path, selected, storage.disk_capacity) {
            Ok(mut partition) => {
                partition_metadata(&mut partition, reads, mounts);
                document
                    .relations
                    .push(Relation::new(format!("Partition {}", partition.name), path));
                storage.partitions.push(partition);
            }
            Err(error) => {
                inventory_failure(
                    document,
                    &mut completed_coverage,
                    "Partition extent",
                    path,
                    error,
                );
            }
        }
        document.storage = Some(storage.clone());
        checkpoint(document);
    }
    storage.coverage = completed_coverage;
    storage.partitions.sort_by_key(|partition| partition.start);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    fn document(path: &Path) -> Document {
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        document.path = path.into();
        document.facts.clear();
        document.relations.clear();
        document
    }
    pub(super) fn storage(path: &Path) -> Storage {
        Storage {
            layout: Layout::Block,
            name: "disk".into(),
            class: "Block disk".into(),
            capacity: Some(512000),
            disk: path.into(),
            disk_capacity: Some(512000),
            partitions: vec![],
            coverage: Outcome::Observed,
            mounts: vec![],
            dependencies: vec![],
            holders: vec![],
            backing: None,
            retained: false,
        }
    }
    #[test]
    fn missing_or_wrong_type_partition_inventory_is_not_a_permission_denial() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file");
        std::fs::write(&file, b"ordinary fixture").unwrap();
        for (disk, expected) in [
            (root.path().join("missing"), Outcome::Missing),
            (file, Outcome::Unsupported),
        ] {
            let mut document = document(&disk);
            let mut storage = storage(&disk);
            let mut reads = Reads::fixture(BTreeMap::new());
            partitions(
                &mut document,
                &disk,
                &disk,
                &mut reads,
                &mut storage,
                &[],
                &mut |_| {},
            );
            assert_eq!(storage.coverage, expected);
            let failure = document
                .facts
                .iter()
                .find(|fact| fact.label == "Partition inventory")
                .unwrap();
            assert_eq!(failure.outcome, expected);
            assert!(failure.error.is_some());
            assert_eq!(reads.attempts().count(), 0);
        }
    }

    #[test]
    fn truncated_partition_map_keeps_its_limit_and_individual_extent_errors() {
        let root = tempfile::tempdir().unwrap();
        let disk = root.path();
        let mut values = BTreeMap::new();
        for index in 0..13 {
            let path = disk.join(format!("part{index:02}"));
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("partition"), b"metadata marker").unwrap();
            for (attribute, value) in [
                ("partition", index + 1),
                ("start", index * 20),
                ("size", 10),
            ] {
                if index != 0 || attribute != "size" {
                    values.insert(path.join(attribute), value.to_string().into_bytes());
                }
            }
        }
        let mut document = document(disk);
        let mut storage = storage(disk);
        let mut reads = Reads::fixture(values);
        partitions(
            &mut document,
            disk,
            disk,
            &mut reads,
            &mut storage,
            &[],
            &mut |_| {},
        );
        assert_eq!(storage.coverage, Outcome::Truncated);
        assert_eq!(storage.partitions.len(), 11);
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "Partition extent" && fact.outcome == Outcome::Missing)
        );
        inventory_failure(
            &mut document,
            &mut storage.coverage,
            "Partition inventory entry",
            disk.into(),
            io::Error::from(io::ErrorKind::PermissionDenied),
        );
        assert_eq!(storage.coverage, Outcome::Truncated);
        assert_eq!(
            document.facts.last().unwrap().outcome,
            Outcome::PermissionDenied
        );
    }

    #[test]
    fn partition_extents_are_proportional_and_never_probe_device_payloads() {
        let temp = tempfile::tempdir().unwrap();
        let disk = temp.path().join("disk");
        std::fs::create_dir(&disk).unwrap();
        let mut inputs = BTreeMap::new();
        for (name, start, size) in [("disk1", 10, 200), ("disk2", 300, 500)] {
            let path = disk.join(name);
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("partition"), b"metadata marker").unwrap();
            for (attribute, value) in [("partition", 1), ("start", start), ("size", size)] {
                inputs.insert(path.join(attribute), value.to_string().into_bytes());
            }
        }
        let mut reads = Reads::fixture(inputs);
        let mut document = document(&disk);
        let mut layout = storage(&disk);
        let mut checkpoints = vec![];
        partitions(
            &mut document,
            &disk,
            &disk.join("disk2"),
            &mut reads,
            &mut layout,
            &[],
            &mut |document| checkpoints.push(document.storage.clone().unwrap()),
        );
        assert_eq!(layout.coverage, Outcome::Observed);
        assert_eq!(layout.partitions.len(), 2);
        assert_eq!(layout.partitions[0].start, 10 * 512);
        assert_eq!(layout.partitions[1].bytes, 500 * 512);
        assert!(layout.partitions[1].selected);
        assert!(
            checkpoints
                .iter()
                .all(|partial| partial.coverage == Outcome::Truncated)
        );
        assert!(reads.attempts().all(|path| matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("partition" | "start" | "size" | "dev")
        )));
    }
    #[test]
    fn malformed_out_of_range_and_overflowing_extents_are_not_drawn() {
        let base = Path::new("/synthetic/disk/part");
        let values = [("partition", "1"), ("start", "900"), ("size", "200")];
        let mut reads = Reads::fixture(
            values
                .into_iter()
                .map(|(name, value)| (base.join(name), value.as_bytes().to_vec()))
                .collect(),
        );
        assert!(partition(&mut reads, base, base, Some(1000 * 512)).is_err());
        let mut reads = Reads::fixture(BTreeMap::from([(
            base.join("size"),
            u64::MAX.to_string().into_bytes(),
        )]));
        assert!(capacity(&mut reads, &base.join("size")).is_err());
    }
    #[test]
    fn loop_attachment_requires_observed_capacity_and_backing_outcome() {
        let path = Path::new("/sys/devices/virtual/block/loop7");
        let mut document = document(path);
        document.facts.push(Fact::failure(
            "Backing file",
            path.join("loop/backing_file"),
            io::Error::from(io::ErrorKind::NotFound),
        ));
        assert_eq!(class(&document, path, Some(0)), "Unattached loop device");
        assert_eq!(class(&document, path, None), "Loop device");
        document.facts[0].outcome = Outcome::PermissionDenied;
        assert_eq!(class(&document, path, Some(0)), "Loop device");
    }
}
