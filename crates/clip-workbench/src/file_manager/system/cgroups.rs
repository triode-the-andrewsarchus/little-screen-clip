//! Read-only cgroup metadata, never process migration or resource configuration.
mod ancestors;
mod legacy;
use super::{
    Document,
    attributes::{self, Format, Rule},
    buses::{self, Topology},
    collector::Reads,
    model::{Fact, Outcome, Value},
};
use little_clip_core::kernel_interface;
use std::{collections::BTreeSet, io, os::fd::AsRawFd, path::Path};

/// Captured hierarchy origin, revalidated before following parent/child links.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(in crate::file_manager) struct HierarchyScope {
    #[serde(with = "super::path_serde")]
    root: std::path::PathBuf,
    identity: super::identity::MountIdentity,
}
impl HierarchyScope {
    pub(super) fn verify(&self, target: &Path) -> Result<(), String> {
        let origin = super::identity::MountIdentity::directory(&self.root)
            .map_err(|error| error.to_string())?;
        let target =
            super::identity::MountIdentity::directory(target).map_err(|error| error.to_string())?;
        if origin != self.identity || !origin.same_hierarchy_mount(target) {
            return Err("Cgroup hierarchy mount view changed; refresh the original object".into());
        }
        Ok(())
    }
}
#[derive(Clone, Copy)]
enum Parser {
    Standard(Format),
    BytesLimit,
    CountLimit,
    CpuLimit,
    Members,
    CpuStats,
    Legacy(legacy::Parser),
}
use Parser::*;
const FIELDS: &[(&str, &str, Parser)] = &[
    ("cgroup.type", "Cgroup type", Standard(Format::Text)),
    (
        "cgroup.controllers",
        "Available controllers",
        Standard(Format::Text),
    ),
    (
        "cgroup.subtree_control",
        "Controllers enabled for children",
        Standard(Format::Text),
    ),
    (
        "cgroup.max.depth",
        "Configured descendant depth limit",
        CountLimit,
    ),
    (
        "cgroup.max.descendants",
        "Configured descendant count limit",
        CountLimit,
    ),
    ("cpu.max", "Local CPU bandwidth limit", CpuLimit),
    (
        "cpu.weight",
        "Local CPU scheduling weight",
        Standard(Format::Unsigned),
    ),
    (
        "cpuset.cpus.effective",
        "Effective CPU membership",
        Standard(Format::CpuList),
    ),
    (
        "cpuset.mems.effective",
        "Effective NUMA membership",
        Standard(Format::CpuList),
    ),
    (
        "memory.current",
        "Memory usage including descendants",
        Standard(Format::Bytes),
    ),
    ("memory.max", "Local hard memory limit", BytesLimit),
    (
        "memory.high",
        "Local memory throttling boundary",
        BytesLimit,
    ),
    (
        "memory.min",
        "Local hard memory protection",
        Standard(Format::Bytes),
    ),
    (
        "memory.low",
        "Local best-effort memory protection",
        Standard(Format::Bytes),
    ),
    (
        "memory.swap.current",
        "Swap usage including descendants",
        Standard(Format::Bytes),
    ),
    ("memory.swap.max", "Local swap limit", BytesLimit),
    (
        "pids.current",
        "Current task count including descendants",
        Standard(Format::Unsigned),
    ),
    ("pids.max", "Local task limit", CountLimit),
    ("cpu.stat", "Accumulated CPU accounting", CpuStats),
    ("cgroup.procs", "Observed process membership", Members),
    ("cgroup.threads", "Observed thread membership", Members),
];
pub(super) fn approved(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            FIELDS
                .iter()
                .chain(legacy::FIELDS)
                .any(|(field, _, _)| name == *field)
        })
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid cgroup metadata")
}
fn unsigned(raw: &str) -> io::Result<u64> {
    raw.parse().map_err(|_| invalid())
}
fn parse(parser: Parser, name: &str, raw: &str) -> io::Result<(Value, String)> {
    match parser {
        Standard(format) => {
            let (semantic, display) = attributes::parse(&Rule::new(name, name, format), raw)?;
            if matches!(
                name,
                "cpuset.mems.effective" | "cpuset.mems" | "cpuset.effective_mems"
            ) {
                Ok((
                    Value::Text(raw.into()),
                    display.replace("logical CPUs", "NUMA nodes"),
                ))
            } else {
                Ok((semantic, display))
            }
        }
        BytesLimit | CountLimit if raw == "max" => Ok((
            Value::Text(raw.into()),
            "No local limit (ancestor constraints may apply)".into(),
        )),
        BytesLimit => attributes::parse(&Rule::new(name, name, Format::Bytes), raw),
        CountLimit => attributes::parse(&Rule::new(name, name, Format::Unsigned), raw),
        CpuLimit => cpu_limit(raw),
        Members => members(raw),
        CpuStats => cpu_stats(raw, CPU_COUNTERS),
        Legacy(parser) => legacy::parse(parser, name, raw),
    }
}
fn cpu_limit(raw: &str) -> io::Result<(Value, String)> {
    let fields: Vec<_> = raw.split_whitespace().collect();
    if fields.len() != 2 {
        return Err(invalid());
    }
    let period = unsigned(fields[1])?;
    if period == 0 {
        return Err(invalid());
    }
    let display = if fields[0] == "max" {
        format!("No local bandwidth limit · period {period} µs (ancestor constraints may apply)")
    } else {
        let quota = unsigned(fields[0])?;
        if quota == 0 {
            return Err(invalid());
        }
        format!(
            "{quota} µs per {period} µs · {:.2} CPU equivalents (local fair-class bandwidth)",
            quota as f64 / period as f64
        )
    };
    Ok((Value::Text(raw.into()), display))
}
fn members(raw: &str) -> io::Result<(Value, String)> {
    let mut ids = BTreeSet::new();
    for field in raw.split_whitespace() {
        let id = field.parse::<u32>().map_err(|_| invalid())?;
        if id == 0 {
            return Err(invalid());
        }
        ids.insert(id);
    }
    let listed = ids
        .iter()
        .take(32)
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let display = format!(
        "{} observed distinct IDs{}{}; changing membership, not an atomic snapshot",
        ids.len(),
        if listed.is_empty() { "" } else { ": " },
        listed
    );
    Ok((
        Value::Text(raw.into()),
        if ids.len() > 32 {
            format!("{display} · first 32 shown")
        } else {
            display
        },
    ))
}
const CPU_COUNTERS: &[(&str, &str, &str)] = &[
    ("usage_usec", "Total CPU time including descendants", " µs"),
    ("user_usec", "User CPU time including descendants", " µs"),
    (
        "system_usec",
        "System CPU time including descendants",
        " µs",
    ),
    ("throttled_usec", "Local bandwidth throttling time", " µs"),
    ("burst_usec", "Local burst time", " µs"),
];
fn cpu_stats(raw: &str, definitions: &[(&str, &str, &str)]) -> io::Result<(Value, String)> {
    if raw.is_empty() {
        return Err(invalid());
    }
    let mut seen = BTreeSet::new();
    let mut display = Vec::new();
    for line in raw.lines().take(65) {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() != 2 || !seen.insert(fields[0]) || seen.len() > 64 {
            return Err(invalid());
        }
        let value = unsigned(fields[1])?;
        let (label, unit) = definitions
            .iter()
            .find(|(key, _, _)| *key == fields[0])
            .map(|(_, label, unit)| (*label, *unit))
            .unwrap_or((fields[0], ""));
        display.push(format!("{label}: {value}{unit}"));
    }
    Ok((Value::Text(raw.into()), display.join(" · ")))
}
fn observe(reads: &mut Reads, root: &Path, field: &(&str, &str, Parser), version: Version) -> Fact {
    let (name, label, parser) = *field;
    let source = root.join(name);
    Fact::attribute(
        label,
        source.clone(),
        format!("documented cgroup {} interface", version.name()),
        reads.cgroup(&source),
        |raw| parse(parser, name, raw),
    )
}

#[derive(Clone, Copy)]
enum Version {
    Unified,
    Legacy,
}
impl Version {
    fn name(self) -> &'static str {
        match self {
            Self::Unified => "v2",
            Self::Legacy => "v1",
        }
    }
    fn fields(self) -> &'static [(&'static str, &'static str, Parser)] {
        match self {
            Self::Unified => FIELDS,
            Self::Legacy => legacy::FIELDS,
        }
    }
}
fn version(root: &Path) -> io::Result<Version> {
    let file = kernel_interface::pin(root, true)?;
    let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: live pinned descriptor and writable statfs storage.
    if unsafe { libc::fstatfs(file.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful fstatfs initialized the structure.
    match unsafe { stat.assume_init() }.f_type as u64 {
        0x63677270 => Ok(Version::Unified),
        0x27e0eb => Ok(Version::Legacy),
        _ => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Not a cgroup filesystem",
        )),
    }
}
pub(super) fn collect(document: &mut Document, root: &Path, checkpoint: &mut dyn FnMut(&Document)) {
    document.title = "Cgroup resource inspector".into();
    match version(root) {
        Ok(version) => collect_version(document, root, &mut Reads::new(), checkpoint, version),
        Err(error) => document
            .facts
            .push(Fact::failure("Cgroup hierarchy", root.into(), error)),
    }
    checkpoint(document);
}
fn collect_version(
    document: &mut Document,
    root: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
    version: Version,
) {
    document.facts.push(Fact::text(
        "Hierarchy position",
        format!(
            "{} · {} · visible namespace and mount view",
            root.display(),
            version.name()
        ),
        root.into(),
    ));
    checkpoint(document);
    if matches!(version, Version::Legacy) {
        legacy::controllers(document, root, reads);
    }
    let requested = document.requested_attribute();
    for field in version
        .fields()
        .iter()
        .filter(|(name, _, _)| requested.as_deref().is_none_or(|request| request == *name))
    {
        document.facts.push(observe(reads, root, field, version));
        checkpoint(document);
    }
    if requested
        .as_deref()
        .is_some_and(|name| !version.fields().iter().any(|(field, _, _)| *field == name))
    {
        document.facts.push(Fact::text(
            "Content policy",
            "Unknown cgroup interface; payload not read",
            document.path.clone(),
        ));
    }
    hierarchy(document, root);
    if requested.is_none() {
        ancestors::collect(document, root, reads, checkpoint, version);
    }
    let policy = match version {
        Version::Unified => {
            "Read-only; no migration or configuration. Local limits may be further constrained by ancestors. Usage includes descendants where documented. Membership IDs are observations, not verified process identities. Process lists are direct membership except threaded-domain lists include their subtree; empty direct membership does not imply an empty subtree."
        }
        Version::Legacy => {
            "Legacy v1; read-only, no migration or configuration. Local limits may be constrained by ancestors. CPU counters are accumulated time, not utilization; memory accounting can be approximate and depends on hierarchical configuration. Memory+swap is a combined value. Very large memory limits may be kernel no-limit sentinels. Membership IDs are non-atomic observations, not verified process identities; empty direct membership does not imply an empty subtree."
        }
    };
    document.facts.push(Fact::text("Cgroup observation policy", format!("{policy} Child inventory: at most 128 entries scanned and 16 retained; no recursive child traversal. Ancestor limit observations are bounded to six visible parents, never computed as available capacity. Parent/child links require the same mount identity; bind-mounted interfaces are not inferred as hierarchy edges."), root.into()));
    checkpoint(document);
}
fn summary(document: &Document, root: &Path) -> String {
    buses::scoped_summary(document, root, |fact| {
        let value = fact.value.as_ref()?;
        match fact.source.file_name().and_then(|name| name.to_str()) {
            Some("memory.current" | "memory.usage_in_bytes") => Some(format!("Memory: {value}")),
            Some("cpu.max" | "cpu.cfs_quota_us") => {
                let bandwidth = if fact
                    .raw
                    .as_deref()
                    .is_some_and(|raw| raw.starts_with("max ") || raw == "-1")
                {
                    "no cap"
                } else {
                    value.split(" · ").next().unwrap_or(value)
                };
                Some(format!("Local CPU: {bandwidth}"))
            }
            _ => None,
        }
    })
}

fn hierarchy(document: &mut Document, root: &Path) {
    let mut topology = Topology::new("Cgroup hierarchy", summary(document, root));
    let scope = match super::identity::MountIdentity::directory(root) {
        Ok(scope) => scope,
        Err(error) => {
            topology.coverage = Outcome::Unsupported;
            document.facts.push(Fact::failure(
                "Hierarchy mount identity",
                root.into(),
                error,
            ));
            document.topology = Some(topology);
            return;
        }
    };
    document.facts.push(Fact::text(
        "Hierarchy mount identity",
        format!(
            "Mount {} in this mount namespace; parent/child links stay on this mount",
            scope.mount
        ),
        root.into(),
    ));
    // Replace generic parent discovery with a cgroup-scoped parent relationship.
    document
        .relations
        .retain(|relation| relation.label != "Parent");
    let parent = root
        .parent()
        .filter(|parent| same_mount(scope, parent))
        .map(|parent| {
            buses::add(
                &mut topology,
                document,
                parent.into(),
                "Parent cgroup".into(),
                None,
            )
        });
    let selected = buses::add(
        &mut topology,
        document,
        root.to_path_buf(),
        "Selected cgroup".into(),
        parent,
    );
    match std::fs::read_dir(root) {
        Ok(entries) => {
            let (children, coverage) = buses::select_children(
                entries,
                |entry| {
                    entry.file_type().is_ok_and(|kind| kind.is_dir())
                        && same_mount(scope, &entry.path())
                },
                16,
            );
            topology.coverage = coverage;
            for child in children {
                let label = child
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                buses::add(
                    &mut topology,
                    document,
                    child,
                    format!("Child cgroup: {label}"),
                    Some(selected),
                );
            }
        }
        Err(error) => {
            topology.coverage = Outcome::Unsupported;
            document
                .facts
                .push(Fact::failure("Child cgroup inventory", root.into(), error));
        }
    }
    for relation in document.relations.iter_mut().filter(|relation| {
        relation.label == "Parent cgroup" || relation.label.starts_with("Child cgroup:")
    }) {
        relation.cgroup_hierarchy = Some(HierarchyScope {
            root: root.into(),
            identity: scope,
        });
    }
    document.topology = Some(topology);
}

fn same_mount(scope: super::identity::MountIdentity, path: &Path) -> bool {
    super::identity::MountIdentity::directory(path)
        .is_ok_and(|other| scope.same_hierarchy_mount(other))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cgroup_limits_preserve_local_and_ancestor_semantics() {
        assert!(cpu_limit("max 100000").unwrap().1.contains("ancestor"));
        assert!(cpu_limit("250000 100000").unwrap().1.contains("2.50"));
        for raw in ["1 0", "-1 100", "0 100", "max", "1 2 3"] {
            assert!(cpu_limit(raw).is_err());
        }
        assert!(
            parse(BytesLimit, "memory.max", "max")
                .unwrap()
                .1
                .contains("No local")
        );
        assert!(
            parse(BytesLimit, "memory.max", "4096")
                .unwrap()
                .1
                .contains("KiB")
        );
        assert!(parse(BytesLimit, "memory.max", "-1").is_err());
    }
    #[test]
    fn membership_is_bounded_display_not_atomic_process_identity() {
        let (_, display) = members("42\n8\n42").unwrap();
        assert!(display.contains("2 observed distinct IDs: 8, 42"));
        assert!(display.contains("not an atomic"));
        assert!(members("0").is_err());
        assert!(members("4294967296").is_err());
        assert!(members("").unwrap().1.contains("0 observed"));
        let raw = (1..=80)
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(members(&raw).unwrap().1.contains("first 32"));
    }
    #[test]
    fn accounting_preserves_known_units_and_rejects_duplicate_or_invalid_fields() {
        let (_, display) =
            cpu_stats("usage_usec 100\nnr_periods 3\nfuture 9", CPU_COUNTERS).unwrap();
        assert!(display.contains("100 µs"));
        assert!(display.contains("future: 9"));
        for raw in [
            "usage_usec -1",
            "usage_usec 1\nusage_usec 2",
            "usage_usec 1 garbage",
        ] {
            assert!(cpu_stats(raw, CPU_COUNTERS).is_err());
        }
    }
    #[test]
    fn approved_metadata_excludes_migration_and_reclaim_payloads() {
        let (temp, root, mut reads, mut document) = super::super::collector::registered_fixture(
            "fixture",
            "service",
            &[
                ("cpu.max", "100000 100000"),
                ("memory.current", "2048"),
                ("cgroup.procs", "8\n9"),
            ],
        );
        document.path = root.clone();
        document.interface_directory = true;
        for index in 0..20 {
            std::fs::create_dir(root.join(format!("child{index}"))).unwrap();
        }
        collect_version(
            &mut document,
            &root,
            &mut reads,
            &mut |_| {},
            Version::Unified,
        );
        assert_eq!(
            document.topology.as_ref().unwrap().coverage,
            Outcome::Truncated
        );
        assert_eq!(
            document
                .relations
                .iter()
                .filter(|link| link.label.starts_with("Child cgroup:"))
                .count(),
            16
        );
        for path in reads.attempts() {
            assert!(approved(path));
        }
        assert!(!approved(&root.join("memory.reclaim")));
        assert!(!approved(&root.join("cgroup.kill")));
        assert!(reads.cgroup(&root.join("memory.reclaim")).is_err());
        document.interface_directory = false;
        document.path = root.join("cpu.max");
        document.facts.clear();
        let mut permissions = Fact::text("Permissions", "0600", root.join("cpu.max"));
        permissions.interface = true;
        document.facts.push(permissions);
        collect_version(
            &mut document,
            &root,
            &mut reads,
            &mut |_| {},
            Version::Unified,
        );
        let summary = &document.topology.as_ref().unwrap().summary;
        assert!(summary.contains("Local CPU"));
        assert!(!summary.contains("Permissions"));
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "Local CPU bandwidth limit")
        );
        assert!(
            !document
                .facts
                .iter()
                .any(|fact| fact.label == "Memory usage including descendants")
        );
        drop(temp);
    }
    #[test]
    fn selected_cgroup_getter_does_not_read_other_local_or_ancestor_payloads() {
        let (_, root, mut reads, mut document) = super::super::collector::registered_fixture(
            "fixture",
            "service",
            &[("cpu.max", "100000 100000")],
        );
        document.path = root.join("cpu.max");
        document.interface_directory = false;
        collect_version(
            &mut document,
            &root,
            &mut reads,
            &mut |_| {},
            Version::Unified,
        );
        assert_eq!(
            reads.attempts().collect::<Vec<_>>(),
            vec![root.join("cpu.max").as_path()]
        );
        assert!(
            !document
                .facts
                .iter()
                .any(|fact| fact.label.starts_with("Ancestor"))
        );
    }

    #[test]
    fn numa_membership_and_oversized_membership_keep_correct_semantics() {
        let (_, display) =
            parse(Standard(Format::CpuList), "cpuset.mems.effective", "0-1,8").unwrap();
        assert!(display.contains("3 NUMA nodes"));
        assert!(!display.contains("CPU"));
        let raw = "12345\n".repeat(1000);
        let root = Path::new("/fixture/group");
        let mut reads = Reads::fixture(std::collections::BTreeMap::from([(
            root.join("cgroup.procs"),
            raw.into_bytes(),
        )]));
        let fact = observe(
            &mut reads,
            root,
            FIELDS
                .iter()
                .find(|(name, _, _)| *name == "cgroup.procs")
                .unwrap(),
            Version::Unified,
        );
        assert_eq!(fact.outcome, Outcome::Truncated);
        assert!(fact.value.is_none());
    }
    #[test]
    fn cgroup_reader_rejects_ordinary_and_other_kernel_filesystems() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cpu.max");
        std::fs::write(&path, "1 2").unwrap();
        assert_eq!(
            Reads::new().cgroup(&path).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
    }
    #[test]
    fn hierarchy_relationships_revalidate_the_origin_mount_and_target_directory() {
        let temp = tempfile::tempdir().unwrap();
        let child = temp.path().join("child");
        std::fs::create_dir(&child).unwrap();
        let identity = super::super::identity::MountIdentity::directory(temp.path()).unwrap();
        let mut scope = HierarchyScope {
            root: temp.path().into(),
            identity,
        };
        scope.verify(&child).unwrap();
        scope.identity.mount += 1;
        assert!(scope.verify(&child).is_err());
        scope.identity = identity;
        let renamed = temp.path().join("original");
        std::fs::rename(&child, &renamed).unwrap();
        std::fs::write(&child, "replaced by file").unwrap();
        assert!(scope.verify(&child).is_err());
    }
}
