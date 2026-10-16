//! MTD geometry and UBI allocation metadata; no flash payloads or scans.
use super::{
    Document, Relation,
    attributes::{self, Context},
    buses::{self, Topology},
    collector::Reads,
    model::{Fact, Outcome},
    storage::{Layout, Partition, Storage},
};
use crate::file_manager::filesystem::format_size;
use std::{
    io,
    path::{Path, PathBuf},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Object {
    Mtd(u32),
    MtdRo(u32),
    MtdBlock(u32),
    Ubi(u32),
    Volume(u32, u32),
    VolumeBlock(u32, u32),
    Control,
}
fn number(raw: &str) -> Option<u32> {
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        None
    } else {
        raw.parse().ok()
    }
}
fn volume(raw: &str) -> Option<(u32, u32)> {
    let (device, volume) = raw.split_once('_')?;
    Some((number(device)?, number(volume)?))
}
fn object(path: &Path) -> Option<Object> {
    let name = path.file_name()?.to_str()?;
    match buses::subsystem(path).as_deref()? {
        "mtd" => {
            let name = name.strip_prefix("mtd")?;
            if let Some(id) = name.strip_suffix("ro") {
                Some(Object::MtdRo(number(id)?))
            } else {
                Some(Object::Mtd(number(name)?))
            }
        }
        "ubi" => {
            let name = name.strip_prefix("ubi")?;
            if let Some(id) = number(name) {
                Some(Object::Ubi(id))
            } else {
                let (id, vol) = volume(name)?;
                Some(Object::Volume(id, vol))
            }
        }
        "block" => block(name),
        "misc" if name == "ubi_ctrl" => Some(Object::Control),
        _ => None,
    }
}
fn block(name: &str) -> Option<Object> {
    if let Some(id) = name.strip_prefix("mtdblock") {
        return Some(Object::MtdBlock(number(id)?));
    }
    let (id, vol) = volume(name.strip_prefix("ubiblock")?)?;
    Some(Object::VolumeBlock(id, vol))
}
struct Classes {
    mtd: PathBuf,
    ubi: PathBuf,
}
fn mapped(classes: &Classes, selected: Object, sys: &Path) -> Option<PathBuf> {
    let path = match selected {
        Object::Mtd(_) | Object::Ubi(_) | Object::Volume(..) => return Some(sys.into()),
        Object::MtdRo(id) | Object::MtdBlock(id) => classes.mtd.join(format!("mtd{id}")),
        Object::VolumeBlock(id, vol) => classes.ubi.join(format!("ubi{id}_{vol}")),
        Object::Control => return None,
    }
    .canonicalize()
    .ok()?;
    match (selected, object(&path)) {
        (Object::MtdRo(a) | Object::MtdBlock(a), Some(Object::Mtd(b))) if a == b => Some(path),
        (Object::VolumeBlock(a, b), Some(Object::Volume(c, d))) if (a, b) == (c, d) => Some(path),
        _ => None,
    }
}
fn fields(document: &mut Document, root: &Path, reads: &mut Reads) {
    for rule in attributes::rules(&Context::discover(root)) {
        reads.observe(document, root, &rule);
    }
}
fn raw(document: &Document, path: &Path, name: &str) -> Option<String> {
    document
        .facts
        .iter()
        .find(|fact| {
            fact.source == path.join(name) && fact.outcome == Outcome::Observed && !fact.retained
        })?
        .raw
        .clone()
}
fn flash_parent(root: &Path) -> Option<PathBuf> {
    root.ancestors()
        .skip(1)
        .find(|parent| matches!(object(parent), Some(Object::Mtd(_))))
        .map(Path::to_path_buf)
}
pub(super) fn collect(
    document: &mut Document,
    sys: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    collect_with(
        document,
        sys,
        reads,
        checkpoint,
        &Classes {
            mtd: "/sys/class/mtd".into(),
            ubi: "/sys/class/ubi".into(),
        },
    );
}
fn collect_with(
    document: &mut Document,
    sys: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
    classes: &Classes,
) {
    let Some(selected) = object(sys) else {
        return;
    };
    document.storage = None;
    if selected == Object::Control {
        document.title = "UBI management interface".into();
        document.facts.push(Fact::text("UBI control semantics","Device-management interface, not a flash volume; no attach/detach operations requested",sys.into()));
        return;
    }
    let Some(root) = mapped(classes, selected, sys) else {
        document.facts.push(Fact::failure(
            "Flash metadata relationship",
            sys.into(),
            io::Error::new(
                io::ErrorKind::NotFound,
                "Matching flash object is unavailable in this sysfs view",
            ),
        ));
        return;
    };
    document
        .relations
        .push(Relation::new("Flash metadata object", root.clone()));
    fields(document, &root, reads);
    if matches!(selected, Object::MtdRo(_)) {
        let mut fact = Fact::text(
            "Read-only flash access interface",
            "Yes; distinct from the underlying kernel write capability",
            sys.into(),
        );
        fact.interface = true;
        document.facts.push(fact);
    }
    match object(&root) {
        Some(Object::Mtd(id)) => {
            mtd(document, &root, reads, checkpoint);
            ubi_for_mtd(document, classes, id, reads);
        }
        Some(Object::Ubi(_) | Object::Volume(..)) => {
            ubi(document, &root, classes, reads, checkpoint)
        }
        _ => {}
    }
    document.title = match selected {
        Object::MtdRo(_) => "MTD read-only access".into(),
        Object::MtdBlock(_) => "MTD block access".into(),
        Object::VolumeBlock(..) => "UBI volume block access".into(),
        _ => document.title.clone(),
    };
    document.facts.push(Fact::text("Flash inspection policy","Geometry, kernel flags and aggregate counters only. No flash read/erase/write, bad-block scan, OOB payload or UBI attach/update operation. Volume reservations are logical allocations, not physical extents or filesystem usage.",sys.into()));
    checkpoint(document);
}
fn writable(document: &mut Document, root: &Path) {
    let Some(raw) = raw(document, root, "flags") else {
        return;
    };
    let source = root.join("flags");
    let mut fact = match u32::from_str_radix(raw.trim_start_matches("0x"), 16) {
        Ok(flags) => Fact::text(
            "Kernel flash write capability",
            if flags & 0x400 == 0 {
                "Read-only"
            } else {
                "Writable; interface permissions/locks remain separate"
            },
            source,
        ),
        Err(_) => Fact::failure(
            "Kernel flash write capability",
            source,
            io::Error::new(io::ErrorKind::InvalidData, "Invalid MTD flags"),
        ),
    };
    fact.origin = "decoded documented MTD flags".into();
    fact.raw = Some(raw);
    document.facts.push(fact);
}
fn mtd(
    document: &mut Document,
    root: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let name = raw(document, root, "name").unwrap_or_else(|| "unnamed flash".into());
    let kind = raw(document, root, "type").unwrap_or_else(|| "type unavailable".into());
    let parent = flash_parent(root);
    document.title = if parent.is_some() {
        "MTD partition"
    } else {
        "MTD flash object"
    }
    .into();
    writable(document, root);
    let capacity = reads.unsigned(&root.join("size")).ok();
    let disk = parent.clone().unwrap_or_else(|| root.into());
    let disk_capacity = reads.unsigned(&disk.join("size")).ok();
    let erase = reads
        .unsigned(&root.join("erasesize"))
        .map(format_size)
        .unwrap_or_else(|_| "unavailable".into());
    let write = reads
        .unsigned(&root.join("writesize"))
        .map(format_size)
        .unwrap_or_else(|_| "unavailable".into());
    let class = format!("MTD {kind} · {name} · major erase {erase} · write {write}");
    let mut storage = Storage {
        layout: Layout::Flash,
        name,
        class,
        capacity,
        disk: disk.clone(),
        disk_capacity,
        partitions: Vec::new(),
        coverage: Outcome::Observed,
        mounts: Vec::new(),
        dependencies: Vec::new(),
        holders: Vec::new(),
        backing: None,
        retained: false,
    };
    if let Some(parent) = parent {
        document
            .relations
            .push(Relation::new("Parent flash object", parent));
    }
    let (paths, coverage) = flash_children(document, &disk);
    storage.coverage = coverage;
    for path in paths {
        match extent(reads, &path, root, disk_capacity) {
            Ok(partition) => {
                document.relations.push(Relation::new(
                    format!("Flash partition {}", partition.name),
                    path,
                ));
                storage.partitions.push(partition);
            }
            Err(error) => {
                let fact = Fact::failure("Flash partition extent", path, error);
                storage.coverage = fact.outcome.clone();
                document.facts.push(fact);
            }
        }
    }
    storage.partitions.sort_by_key(|partition| partition.start);
    document.storage = Some(storage);
    checkpoint(document);
}
fn flash_children(document: &mut Document, root: &Path) -> (Vec<PathBuf>, Outcome) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            document.facts.push(Fact::failure(
                "Flash partition inventory",
                root.into(),
                error,
            ));
            return (Vec::new(), Outcome::Unsupported);
        }
    };
    let (paths, coverage) = buses::select_children(
        entries,
        |entry| {
            entry.file_type().is_ok_and(|kind| kind.is_dir())
                && matches!(object(&entry.path()), Some(Object::Mtd(_)))
                && entry.path().join("offset").is_file()
        },
        12,
    );
    if coverage != Outcome::Observed {
        let mut fact = Fact::text(
            "Flash partition inventory",
            "Bounded inventory is incomplete",
            root.into(),
        );
        fact.outcome = coverage.clone();
        document.facts.push(fact);
    }
    (paths, coverage)
}
fn extent(
    reads: &mut Reads,
    path: &Path,
    selected: &Path,
    capacity: Option<u64>,
) -> io::Result<Partition> {
    let start = reads.unsigned(&path.join("offset"))?;
    let bytes = reads.unsigned(&path.join("size"))?;
    let end = start
        .checked_add(bytes)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Flash extent overflow"))?;
    let capacity = capacity.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "Parent flash capacity unavailable",
        )
    })?;
    if bytes == 0 || end > capacity {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Flash partition outside parent capacity",
        ));
    }
    let name = reads.read(&path.join("name")).unwrap_or_else(|_| {
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    });
    Ok(Partition {
        name,
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
fn ubi_for_mtd(document: &mut Document, classes: &Classes, id: u32, reads: &mut Reads) {
    let Ok(entries) = std::fs::read_dir(&classes.ubi) else {
        return;
    };
    let (paths, coverage) = buses::select_children(
        entries,
        |entry| matches!(object(&entry.path()), Some(Object::Ubi(_))),
        16,
    );
    for path in paths {
        let path = match path.canonicalize() {
            Ok(path) => path,
            Err(error) => {
                document
                    .facts
                    .push(Fact::failure("UBI association", path, error));
                continue;
            }
        };
        if reads.unsigned(&path.join("mtd_num")).ok() == Some(u64::from(id)) {
            document
                .relations
                .push(Relation::new("Associated UBI device", path));
        }
    }
    if coverage != Outcome::Observed {
        let mut fact = Fact::text(
            "UBI association inventory",
            "Bounded inventory is incomplete",
            classes.ubi.clone(),
        );
        fact.outcome = coverage;
        document.facts.push(fact);
    }
}
fn volume_capacity(reads: &mut Reads, root: &Path) -> io::Result<u64> {
    reads
        .unsigned(&root.join("reserved_ebs"))?
        .checked_mul(reads.unsigned(&root.join("eraseblock_size"))?)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "UBI logical reservation overflow",
            )
        })
}
fn ubi(
    document: &mut Document,
    root: &Path,
    classes: &Classes,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let selected = object(root);
    document.title = if matches!(selected, Some(Object::Volume(..))) {
        "UBI volume"
    } else {
        "UBI device"
    }
    .into();
    let controller = if let Some(Object::Volume(id, _)) = selected {
        root.ancestors()
            .skip(1)
            .find(|path| object(path) == Some(Object::Ubi(id)))
            .map(Path::to_path_buf)
            .or_else(|| classes.ubi.join(format!("ubi{id}")).canonicalize().ok())
    } else {
        Some(root.into())
    };
    let Some(controller) = controller else {
        return;
    };
    fields(document, &controller, reads);
    let number = reads.unsigned(&controller.join("mtd_num"));
    let mut summary = format!(
        "{} · backing MTD unavailable",
        root.file_name().unwrap_or_default().to_string_lossy()
    );
    let mut topology = Topology::new("UBI logical volumes", summary.clone());
    let mut parent = None;
    if let Ok(number) = number {
        let mtd = classes.mtd.join(format!("mtd{number}")).canonicalize().ok();
        if let Some(mtd) =
            mtd.filter(|mtd| matches!(object(mtd),Some(Object::Mtd(id)) if u64::from(id)==number))
        {
            parent = Some(buses::add(
                &mut topology,
                document,
                mtd.clone(),
                format!("Backing mtd{number}"),
                None,
            ));
            summary = format!(
                "{} · backed by mtd{number}",
                root.file_name().unwrap_or_default().to_string_lossy()
            );
        }
    }
    let anchor = buses::add(
        &mut topology,
        document,
        controller.clone(),
        "UBI controller".into(),
        parent,
    );
    if root != controller {
        add_volume(&mut topology, document, root, reads, anchor);
    }
    let entries = std::fs::read_dir(&controller);
    match entries {
        Ok(entries) => {
            let (paths, coverage) = buses::select_children(
                entries,
                |entry| matches!(object(&entry.path()), Some(Object::Volume(..))),
                16,
            );
            topology.coverage = coverage;
            for path in paths {
                if path != root {
                    add_volume(&mut topology, document, &path, reads, anchor);
                }
            }
        }
        Err(error) => {
            document.facts.push(Fact::failure(
                "UBI volume inventory",
                controller.clone(),
                error,
            ));
            topology.coverage = Outcome::Unsupported;
        }
    }
    topology.summary = summary;
    document.topology = Some(topology);
    checkpoint(document);
}
fn add_volume(
    topology: &mut Topology,
    document: &mut Document,
    root: &Path,
    reads: &mut Reads,
    parent: usize,
) {
    let name = reads
        .read(&root.join("name"))
        .unwrap_or_else(|_| "name unavailable".into());
    let kind = reads
        .read(&root.join("type"))
        .unwrap_or_else(|_| "type unavailable".into());
    let capacity = match volume_capacity(reads, root) {
        Ok(size) => format_size(size),
        Err(error) => {
            document
                .facts
                .push(Fact::failure("UBI reserved capacity", root.into(), error));
            "capacity unavailable".into()
        }
    };
    buses::add(
        topology,
        document,
        root.into(),
        format!("{name} · {kind} · {capacity} reserved"),
        Some(parent),
    );
    buses::access_node(document, root, "UBI volume access");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, os::unix::fs::symlink};
    struct Fixture {
        _dir: tempfile::TempDir,
        classes: Classes,
        master: PathBuf,
        partition: PathBuf,
        ubi: PathBuf,
        volume: PathBuf,
        values: BTreeMap<PathBuf, Vec<u8>>,
    }
    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let mtd_bus = dir.path().join("mtd-bus/mtd");
            let ubi_bus = dir.path().join("ubi-bus/ubi");
            std::fs::create_dir_all(&mtd_bus).unwrap();
            std::fs::create_dir_all(&ubi_bus).unwrap();
            let classes = Classes {
                mtd: dir.path().join("class/mtd"),
                ubi: dir.path().join("class/ubi"),
            };
            std::fs::create_dir_all(&classes.mtd).unwrap();
            std::fs::create_dir_all(&classes.ubi).unwrap();
            let master = dir.path().join("controller/mtd0");
            let partition = master.join("mtd3");
            let ubi = dir.path().join("virtual/ubi0");
            let volume = ubi.join("ubi0_1");
            for (path, bus) in [
                (&master, &mtd_bus),
                (&partition, &mtd_bus),
                (&ubi, &ubi_bus),
                (&volume, &ubi_bus),
            ] {
                std::fs::create_dir_all(path).unwrap();
                symlink(bus, path.join("subsystem")).unwrap();
            }
            symlink(&master, classes.mtd.join("mtd0")).unwrap();
            symlink(&partition, classes.mtd.join("mtd3")).unwrap();
            symlink(&ubi, classes.ubi.join("ubi0")).unwrap();
            symlink(&volume, classes.ubi.join("ubi0_1")).unwrap();
            std::fs::write(partition.join("offset"), "metadata marker").unwrap();
            let mut values = BTreeMap::new();
            for (path, fields) in [
                (
                    &master,
                    vec![
                        ("name", "chip"),
                        ("type", "nand"),
                        ("size", "16384"),
                        ("erasesize", "1024"),
                        ("writesize", "256"),
                        ("flags", "0x400"),
                    ],
                ),
                (
                    &partition,
                    vec![
                        ("name", "rootfs"),
                        ("type", "nand"),
                        ("size", "8192"),
                        ("offset", "4096"),
                        ("flags", "0x0"),
                        ("bad_blocks", "2"),
                    ],
                ),
                (
                    &ubi,
                    vec![
                        ("mtd_num", "3"),
                        ("total_eraseblocks", "8"),
                        ("avail_eraseblocks", "2"),
                        ("eraseblock_size", "768"),
                        ("max_ec", "20"),
                    ],
                ),
                (
                    &volume,
                    vec![
                        ("name", "root-volume"),
                        ("type", "dynamic"),
                        ("reserved_ebs", "4"),
                        ("eraseblock_size", "768"),
                        ("data_bytes", "3072"),
                    ],
                ),
            ] {
                for (name, value) in fields {
                    values.insert(path.join(name), value.as_bytes().to_vec());
                }
            }
            Self {
                _dir: dir,
                classes,
                master,
                partition,
                ubi,
                volume,
                values,
            }
        }
        fn document(&self) -> Document {
            let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
            document.facts.clear();
            document.relations.clear();
            document
        }
    }
    #[test]
    fn flash_oob_ecc_and_wear_metadata_keep_native_units_and_sources() {
        let fixture = Fixture::new();
        for (root, counters, sizes) in [
            (
                &fixture.partition,
                vec![
                    "ecc_strength",
                    "ecc_failures",
                    "corrected_bits",
                    "bad_blocks",
                    "bbt_blocks",
                    "numeraseregions",
                ],
                vec!["oobsize", "oobavail", "ecc_step_size"],
            ),
            (
                &fixture.ubi,
                vec![
                    "total_eraseblocks",
                    "avail_eraseblocks",
                    "max_ec",
                    "bad_peb_count",
                    "reserved_for_bad",
                    "volumes_count",
                ],
                vec!["eraseblock_size", "min_io_size"],
            ),
        ] {
            let values = counters
                .iter()
                .map(|name| (root.join(name), b"7".to_vec()))
                .chain(sizes.iter().map(|name| (root.join(name), b"4096".to_vec())))
                .collect();
            let mut document = fixture.document();
            let mut reads = Reads::fixture(values);
            fields(&mut document, root, &mut reads);
            for (names, raw, display, number) in
                [(counters, "7", "7", 7), (sizes, "4096", "4.0 KiB", 4096)]
            {
                for name in names {
                    let fact = document
                        .facts
                        .iter()
                        .find(|fact| fact.source == root.join(name))
                        .unwrap();
                    assert_eq!(fact.outcome, Outcome::Observed);
                    assert_eq!(fact.raw.as_deref(), Some(raw));
                    assert_eq!(fact.value.as_deref(), Some(display));
                    assert_eq!(
                        fact.semantic,
                        Some(super::super::model::Value::Integer(number))
                    );
                }
            }
            assert!(
                reads
                    .attempts()
                    .all(|path| path.parent() == Some(root.as_path()))
            );
        }
    }
    #[test]
    fn invalid_or_missing_flash_statistics_do_not_become_zero_or_valid_geometry() {
        let fixture = Fixture::new();
        for (root, invalid, absent) in [
            (&fixture.partition, "ecc_failures", "oobavail"),
            (&fixture.ubi, "max_ec", "bad_peb_count"),
        ] {
            for raw in ["-1", "", "18446744073709551616", "1.5"] {
                let mut document = fixture.document();
                let mut reads = Reads::fixture(BTreeMap::from([(
                    root.join(invalid),
                    raw.as_bytes().to_vec(),
                )]));
                fields(&mut document, root, &mut reads);
                for (name, expected) in [(invalid, Outcome::Malformed), (absent, Outcome::Missing)]
                {
                    let fact = document
                        .facts
                        .iter()
                        .find(|fact| fact.source == root.join(name))
                        .unwrap();
                    assert_eq!(fact.outcome, expected);
                    assert!(fact.semantic.is_none());
                    assert!(fact.value.is_none());
                }
            }
        }
    }
    #[test]
    fn flash_partitions_use_observed_parent_relative_byte_ranges_and_no_payloads() {
        let fixture = Fixture::new();
        let mut document = fixture.document();
        let mut reads = Reads::fixture(fixture.values.clone());
        collect_with(
            &mut document,
            &fixture.partition,
            &mut reads,
            &mut |_| {},
            &fixture.classes,
        );
        let storage = document.storage.as_ref().unwrap();
        assert_eq!(storage.layout, Layout::Flash);
        assert_eq!(storage.disk, fixture.master);
        assert_eq!(storage.disk_capacity, Some(16384));
        assert_eq!(storage.partitions.len(), 1);
        assert_eq!(storage.partitions[0].start, 4096);
        assert_eq!(storage.partitions[0].bytes, 8192);
        assert!(storage.partitions[0].selected);
        assert!(
            document
                .relations
                .iter()
                .any(|relation| relation.label == "Associated UBI device"
                    && relation.target == fixture.ubi)
        );
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "Kernel flash write capability"
                    && fact.value.as_deref() == Some("Read-only"))
        );
        assert!(reads.attempts().all(|path| !matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some("data" | "oob" | "erase" | "mem")
        )));
    }
    #[test]
    fn ubi_reservations_have_no_invented_physical_offsets_or_filesystem_usage() {
        let fixture = Fixture::new();
        let mut document = fixture.document();
        let mut reads = Reads::fixture(fixture.values.clone());
        collect_with(
            &mut document,
            &fixture.volume,
            &mut reads,
            &mut |_| {},
            &fixture.classes,
        );
        assert!(document.storage.is_none());
        let topology = document.topology.as_ref().unwrap();
        assert!(
            topology
                .nodes
                .iter()
                .any(|node| node.path == fixture.partition)
        );
        assert!(
            topology
                .nodes
                .iter()
                .any(|node| node.label.contains("root-volume")
                    && node.label.contains("3.0 KiB reserved"))
        );
        assert_eq!(volume_capacity(&mut reads, &fixture.volume).unwrap(), 3072);
        let mut values = fixture.values.clone();
        values.insert(
            fixture.volume.join("reserved_ebs"),
            u64::MAX.to_string().into_bytes(),
        );
        assert!(volume_capacity(&mut Reads::fixture(values), &fixture.volume).is_err());
    }
    #[test]
    fn malformed_flash_extents_and_missing_inventory_are_explicit() {
        let fixture = Fixture::new();
        let mut values = fixture.values.clone();
        values.insert(
            fixture.partition.join("offset"),
            u64::MAX.to_string().into_bytes(),
        );
        assert!(
            extent(
                &mut Reads::fixture(values),
                &fixture.partition,
                &fixture.partition,
                Some(16384)
            )
            .is_err()
        );
        assert!(
            extent(
                &mut Reads::fixture(fixture.values.clone()),
                &fixture.partition,
                &fixture.partition,
                None
            )
            .is_err()
        );
        let mut document = fixture.document();
        let (_, coverage) = flash_children(&mut document, &fixture.master.join("missing"));
        assert_eq!(coverage, Outcome::Unsupported);
        let rule = attributes::rule(&Context::discover(&fixture.partition), "size").unwrap();
        assert!(attributes::parse(&rule, "-1").is_err());
    }
    #[test]
    fn unavailable_large_backing_id_does_not_create_a_phantom_mtd_link() {
        let fixture = Fixture::new();
        let mut values = fixture.values.clone();
        values.insert(
            fixture.ubi.join("mtd_num"),
            u64::MAX.to_string().into_bytes(),
        );
        let mut document = fixture.document();
        collect_with(
            &mut document,
            &fixture.ubi,
            &mut Reads::fixture(values),
            &mut |_| {},
            &fixture.classes,
        );
        assert!(
            !document
                .topology
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .any(|node| node.label.starts_with("Backing mtd"))
        );
    }
    #[test]
    fn read_only_alias_keeps_interface_semantics_separate_from_geometry_flags() {
        let fixture = Fixture::new();
        let mut values = fixture.values.clone();
        values.insert(fixture.partition.join("flags"), b"0x400".to_vec());
        let ro = fixture.master.join("mtd3ro");
        std::fs::create_dir(&ro).unwrap();
        symlink(
            fixture
                .classes
                .mtd
                .join("mtd3/subsystem")
                .canonicalize()
                .unwrap(),
            ro.join("subsystem"),
        )
        .unwrap();
        let mut document = fixture.document();
        collect_with(
            &mut document,
            &ro,
            &mut Reads::fixture(values),
            &mut |_| {},
            &fixture.classes,
        );
        assert_eq!(document.title, "MTD read-only access");
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "Kernel flash write capability"
                    && fact.value.as_deref().unwrap_or("").starts_with("Writable"))
        );
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "Read-only flash access interface" && fact.interface)
        );
        assert_eq!(document.storage.as_ref().unwrap().capacity, Some(8192));
    }
    #[test]
    fn block_access_aliases_resolve_to_geometry_without_payload_reads() {
        let fixture = Fixture::new();
        let block = fixture._dir.path().join("block");
        std::fs::create_dir(&block).unwrap();
        for (name, title) in [
            ("mtdblock3", "MTD block access"),
            ("ubiblock0_1", "UBI volume block access"),
        ] {
            let path = fixture._dir.path().join(name);
            std::fs::create_dir(&path).unwrap();
            symlink(&block, path.join("subsystem")).unwrap();
            let mut document = fixture.document();
            let mut reads = Reads::fixture(fixture.values.clone());
            collect_with(
                &mut document,
                &path,
                &mut reads,
                &mut |_| {},
                &fixture.classes,
            );
            assert_eq!(document.title, title);
            assert!(reads.attempts().all(|source| source != path));
        }
    }
    #[test]
    fn proportional_flash_rendering_matches_observed_geometry_in_both_appearances() {
        use little_clip_core::{config::WorkbenchAppearance, palette::AppPalette};
        let fixture = Fixture::new();
        let mut document = fixture.document();
        collect_with(
            &mut document,
            &fixture.partition,
            &mut Reads::fixture(fixture.values.clone()),
            &mut |_| {},
            &fixture.classes,
        );
        let storage = document.storage.as_ref().unwrap();
        for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
            let palette = AppPalette::for_appearance(appearance);
            let colors = crate::ui_appearance::WorkbenchColors::from_palette(&palette);
            let context = egui::Context::default();
            let output = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(800.0, 600.0),
                    )),
                    ..Default::default()
                },
                |context| {
                    egui::CentralPanel::default().show(context, |ui| {
                        super::super::storage::paint(
                            ui,
                            egui::Rect::from_min_size(
                                egui::pos2(20.0, 20.0),
                                egui::vec2(256.0, 42.0),
                            ),
                            storage,
                            colors,
                        );
                    });
                },
            );
            assert!(output.shapes.iter().any(|shape| match &shape.shape {
                egui::epaint::Shape::Rect(rect) =>
                    rect.fill == colors.accent
                        && (rect.rect.left() - 84.0).abs() < 0.01
                        && (rect.rect.width() - 128.0).abs() < 0.01,
                _ => false,
            }));
        }
    }
}
