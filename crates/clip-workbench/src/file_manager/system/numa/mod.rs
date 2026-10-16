//! NUMA membership and relative distance snapshots; no migration or hotplug controls.
mod ui;
use super::{
    Document, Relation,
    attributes::{self, Format, Rule},
    buses,
    collector::Reads,
    model::{Fact, Outcome, Value},
};
use serde::{Deserialize, Serialize};
use std::{io, path::Path};
pub(super) use ui::{inspector, paint};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::file_manager) struct Row {
    pub id: u32,
    pub values: Option<Vec<u16>>,
    pub observed_ms: u64,
    pub retained: bool,
    pub outcome: Outcome,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::file_manager) struct Numa {
    pub summary: String,
    pub columns: Vec<u32>,
    pub rows: Vec<Row>,
    pub coverage: Outcome,
    #[serde(default)]
    pub verified_mapping: bool,
}
impl Numa {
    pub(super) fn retain(&mut self, previous: &Self) {
        if !self.verified_mapping || self.columns != previous.columns {
            return;
        }
        for row in self.rows.iter_mut().filter(|row| row.values.is_none()) {
            if let Some(old) = previous
                .rows
                .iter()
                .find(|old| old.id == row.id && old.values.is_some())
            {
                row.values = old.values.clone();
                row.observed_ms = old.observed_ms;
                row.retained = true;
            }
        }
    }
}
fn node_id(path: &Path) -> Option<u32> {
    let name = path.file_name()?.to_str()?.strip_prefix("node")?;
    if name.is_empty()
        || !name.bytes().all(|b| b.is_ascii_digit())
        || buses::subsystem(path).as_deref() != Some("node")
    {
        return None;
    }
    name.parse().ok()
}
pub(super) fn is_root(path: &Path) -> bool {
    super::boot_firmware::same_object(path, Path::new("/sys/devices/system/node"))
}
pub(super) fn rules(path: &Path) -> Vec<Rule> {
    let specs: &[(&str, &str, Format)] = if node_id(path).is_some() {
        &[
            ("cpulist", "NUMA online CPU membership", Format::CpuList),
            ("meminfo", "NUMA memory information", Format::Text),
            ("distance", "NUMA relative distances", Format::Text),
        ]
    } else if is_root(path) {
        &[
            ("online", "Online NUMA nodes", Format::Text),
            ("possible", "Possible NUMA nodes", Format::Text),
            ("has_cpu", "NUMA nodes with CPUs", Format::Text),
            ("has_memory", "NUMA nodes with memory", Format::Text),
        ]
    } else {
        &[]
    };
    attributes::materialize_specs(specs)
}
fn ids(raw: &str) -> io::Result<Vec<u32>> {
    let ranges = attributes::cpu_list(raw)?;
    let count: u64 = ranges.iter().map(|(a, b)| u64::from(b - a) + 1).sum();
    if count > 256 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Online node set exceeds 256-node distance mapping budget",
        ));
    }
    Ok(ranges.into_iter().flat_map(|(a, b)| a..=b).collect())
}
fn distances(raw: &str, columns: &[u32]) -> io::Result<Vec<u16>> {
    let values = raw
        .split_whitespace()
        .map(|v| {
            v.parse::<u16>().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "Invalid relative NUMA distance")
            })
        })
        .collect::<io::Result<Vec<_>>>()?;
    if values.len() != columns.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Distance column count differs from observed online-node set; mapping unverified",
        ));
    }
    Ok(values)
}
fn memory(raw: &str, id: u32, key: &str) -> io::Result<u64> {
    let prefix = format!("Node {id} {key}:");
    let mut matching = raw.lines().filter_map(|line| line.strip_prefix(&prefix));
    let line = matching
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "Memory field not exposed"))?;
    if matching.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Duplicate node memory field",
        ));
    }
    let fields = line.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 2 || fields[1] != "kB" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Unexpected NUMA memory unit",
        ));
    }
    fields[0]
        .parse::<u64>()
        .ok()
        .and_then(|v| v.checked_mul(1024))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "Node memory byte count overflow or malformed",
            )
        })
}
pub(super) fn collect(
    document: &mut Document,
    sys: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    collect_at(
        document,
        sys,
        Path::new("/sys/devices/system/node"),
        reads,
        checkpoint,
    );
}
fn collect_at(
    document: &mut Document,
    sys: &Path,
    reference: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let selected = node_id(sys);
    let root = if selected.is_some() {
        let Some(parent) = sys.parent() else {
            return;
        };
        if !super::boot_firmware::same_object(parent, reference) {
            return;
        }
        parent
    } else if super::boot_firmware::same_object(sys, reference) {
        sys
    } else {
        return;
    };
    let mut data = Numa {
        summary: "NUMA topology".into(),
        columns: Vec::new(),
        rows: Vec::new(),
        coverage: Outcome::Observed,
        verified_mapping: false,
    };
    match reads.read(&root.join("online")).and_then(|raw| ids(&raw)) {
        Ok(columns) => data.columns = columns,
        Err(error) => {
            let fact = Fact::failure("NUMA distance column mapping", root.join("online"), error);
            data.coverage = fact.outcome.clone();
            document.facts.push(fact);
        }
    }
    let (paths, coverage) = match selected {
        Some(_) => (vec![sys.to_path_buf()], Outcome::Observed),
        None => match std::fs::read_dir(root) {
            Ok(entries) => {
                buses::select_children(entries, |entry| node_id(&entry.path()).is_some(), 16)
            }
            Err(error) => {
                document
                    .facts
                    .push(Fact::failure("NUMA node inventory", root.into(), error));
                (Vec::new(), Outcome::Unsupported)
            }
        },
    };
    if coverage != Outcome::Observed {
        data.coverage = coverage.clone();
    }
    if coverage == Outcome::Truncated {
        let mut fact = Fact::text(
            "NUMA node inventory",
            format!(
                "{} registered node rows retained; at most 128 entries scanned and 16 retained",
                paths.len()
            ),
            root.into(),
        );
        fact.outcome = coverage;
        document.facts.push(fact);
    }
    document.title = "NUMA node topology".into();
    for path in paths {
        let Some(id) = node_id(&path) else {
            continue;
        };
        for rule in rules(&path) {
            reads.observe(document, &path, &rule);
        }
        let row = distance_row(document, &path, id, &data.columns);
        if row.outcome != Outcome::Observed && data.coverage != Outcome::Truncated {
            data.coverage = row.outcome.clone();
        }
        node_facts(document, &path, id, selected.is_some(), &mut data.summary);
        data.rows.push(row);
        let mut pending = data.clone();
        for row in &mut pending.rows {
            row.values = None;
            row.outcome = Outcome::NotRequested;
        }
        pending.coverage = Outcome::NotRequested;
        document.numa = Some(pending);
        checkpoint(document);
    }
    validate_mapping(document, root, reads, &mut data);
    if selected.is_none() {
        data.summary = format!(
            "NUMA · {} represented nodes · {} online distance columns · {}",
            data.rows.len(),
            data.columns.len(),
            data.coverage
        );
    }
    document.facts.push(Fact::text("NUMA observation policy","Relative distances are not measured latency or bandwidth. CPU lists reflect exposed online membership; memory and distance observations are sequential, not an atomic hotplug snapshot. No compaction, reclaim, hotplug or memory migration",root.into()));
    document.numa = Some(data);
    checkpoint(document);
}
fn distance_row(document: &Document, path: &Path, id: u32, columns: &[u32]) -> Row {
    let source = path.join("distance");
    let fact = document.facts.iter().find(|fact| fact.source == source);
    let values = fact
        .filter(|fact| fact.outcome == Outcome::Observed && !fact.retained)
        .and_then(|fact| fact.raw.as_deref())
        .map(|raw| distances(raw, columns));
    let outcome = match &values {
        Some(Ok(_)) => Outcome::Observed,
        Some(Err(_)) => Outcome::Malformed,
        None => fact.map_or(Outcome::Missing, |fact| {
            if fact.outcome == Outcome::Observed {
                Outcome::NotRequested
            } else {
                fact.outcome.clone()
            }
        }),
    };
    Row {
        id,
        values: values.and_then(Result::ok),
        observed_ms: fact.map_or(document.observed_ms, |fact| fact.observed_ms),
        retained: false,
        outcome,
    }
}

fn node_facts(document: &mut Document, path: &Path, id: u32, selected: bool, summary: &mut String) {
    if selected {
        *summary = format!("node{id}");
    }
    if let Some(fact) = document.facts.iter().find(|fact| {
        fact.source == path.join("cpulist") && fact.outcome == Outcome::Observed && !fact.retained
    }) {
        if selected {
            if let Some(raw) = &fact.raw {
                summary.push_str(&format!(" · CPUs {raw}"));
            }
        }
    }
    let meminfo = document
        .facts
        .iter()
        .find(|fact| {
            fact.source == path.join("meminfo")
                && fact.outcome == Outcome::Observed
                && !fact.retained
        })
        .cloned()
        .and_then(|mut fact| fact.raw.take().map(|raw| (fact, raw)));
    if let Some((meminfo, raw)) = meminfo {
        for (key, label) in [("MemTotal", "Total"), ("MemFree", "Free")] {
            let mut fact = match memory(&raw, id, key) {
                Ok(bytes) => {
                    let value = super::super::filesystem::format_size(bytes);
                    if selected {
                        summary.push_str(&format!(" · {label} {value}"));
                    }
                    let mut fact = Fact::text(
                        format!("node{id} {label} memory"),
                        value,
                        path.join("meminfo"),
                    );
                    fact.semantic = Some(Value::Integer(bytes.into()));
                    fact.raw = Some(bytes.to_string());
                    fact.origin = format!("decoded node meminfo {key} (kB to bytes)");
                    fact
                }
                Err(error) => Fact::failure(
                    format!("node{id} {label} memory"),
                    path.join("meminfo"),
                    error,
                ),
            };
            fact.inherit_observation(&meminfo);
            document.facts.push(fact);
        }
    }
    document
        .relations
        .push(Relation::new(format!("NUMA node {id}"), path.into()));
    for prefix in ["cpu", "memory"] {
        links(document, path, id, prefix);
    }
}

fn validate_mapping(document: &mut Document, root: &Path, reads: &mut Reads, data: &mut Numa) {
    data.verified_mapping = false;
    let after = reads
        .read_fresh(&root.join("online"))
        .and_then(|raw| ids(&raw));
    if after.as_ref().is_ok_and(|after| *after == data.columns)
        && data.coverage != Outcome::NotRequested
    {
        data.verified_mapping = true;
        for row in &data.rows {
            if let Some(values) = &row.values {
                let text = data
                    .columns
                    .iter()
                    .zip(values)
                    .map(|(id, value)| format!("node{id}: {value}"))
                    .collect::<Vec<_>>()
                    .join(" · ");
                let mut fact = Fact::text(
                    format!("node{} distance column mapping", row.id),
                    text,
                    root.join(format!("node{}/distance", row.id)),
                );
                fact.origin="relative distance row paired with online-node order observed before and after collection".into();
                fact.observed_ms = row.observed_ms;
                if let Some(source) = document
                    .facts
                    .iter()
                    .find(|source| source.source == fact.source)
                {
                    fact.inherit_observation(source);
                }
                document.facts.push(fact);
            }
        }
    } else {
        let error = after.err().unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "Online-node membership changed during collection; distance mapping unverified",
            )
        });
        let fact = Fact::failure(
            "NUMA distance mapping validation",
            root.join("online"),
            error,
        );
        if data.coverage != Outcome::Truncated {
            data.coverage = fact.outcome.clone();
        }
        for row in &mut data.rows {
            row.values = None;
            row.outcome = fact.outcome.clone();
        }
        document.facts.push(fact);
    }
}

fn links(document: &mut Document, path: &Path, id: u32, prefix: &str) {
    let result = std::fs::read_dir(path).map(|entries| {
        buses::select_children(
            entries,
            |entry| {
                entry.file_type().is_ok_and(|kind| kind.is_symlink())
                    && entry
                        .file_name()
                        .to_str()
                        .and_then(|name| name.strip_prefix(prefix))
                        .is_some_and(|suffix| {
                            !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit())
                        })
            },
            8,
        )
    });
    match result {
        Ok((paths, coverage)) => {
            let mut fact = Fact::text(
                format!("node{id} {prefix} relationship coverage"),
                coverage.to_string(),
                path.into(),
            );
            fact.outcome = coverage;
            document.facts.push(fact);
            for link in paths {
                if let Ok(target) = link.canonicalize() {
                    document.relations.push(Relation::new(
                        format!(
                            "node{id} {}",
                            link.file_name().unwrap_or_default().to_string_lossy()
                        ),
                        target,
                    ));
                }
            }
        }
        Err(error) => document.facts.push(Fact::failure(
            format!("node{id} {prefix} relationship coverage"),
            path.into(),
            error,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, os::unix::fs::symlink};
    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, Reads, Document) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("node");
        let bus = temp.path().join("bus/node");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&bus).unwrap();
        let mut values = BTreeMap::from([(root.join("online"), b"0,7".to_vec())]);
        for (id, cpus, dist) in [(0, "0-3", "10 32"), (7, "8-11", "40 10")] {
            let node = root.join(format!("node{id}"));
            std::fs::create_dir(&node).unwrap();
            symlink(&bus, node.join("subsystem")).unwrap();
            values.insert(node.join("cpulist"), cpus.as_bytes().to_vec());
            values.insert(node.join("distance"), dist.as_bytes().to_vec());
            values.insert(
                node.join("meminfo"),
                format!("Node {id} MemTotal: 1024 kB\nNode {id} MemFree: 256 kB\n").into_bytes(),
            );
        }
        let doc = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        (temp, root, Reads::fixture(values), doc)
    }
    #[test]
    fn capped_node_inventory_keeps_truncation_when_later_distance_rows_are_missing() {
        let (_temp, root, mut reads, mut document) = fixture();
        let subsystem = root.join("node0/subsystem").canonicalize().unwrap();
        for id in 1..21 {
            if id == 7 {
                continue;
            }
            let node = root.join(format!("node{id}"));
            std::fs::create_dir(&node).unwrap();
            symlink(&subsystem, node.join("subsystem")).unwrap();
        }
        collect_at(&mut document, &root, &root, &mut reads, &mut |_| {});
        let data = document.numa.as_ref().unwrap();
        assert_eq!(data.rows.len(), 16);
        assert_eq!(data.coverage, Outcome::Truncated);
        assert!(data.summary.contains("Truncated"));
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "NUMA node inventory"
                    && fact.outcome == Outcome::Truncated
                    && fact.value.as_deref().unwrap().contains("16 retained"))
        );
        assert!(data.rows.iter().any(|row| row.outcome == Outcome::Missing));
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.source.ends_with("distance") && fact.outcome == Outcome::Missing)
        );
    }

    #[test]
    fn failed_mapping_validation_keeps_inventory_cap_and_typed_row_failure() {
        let (_temp, root, mut reads, mut document) = fixture();
        collect_at(&mut document, &root, &root, &mut reads, &mut |_| {});
        let previous = document.numa.take().unwrap();
        let mut current = previous.clone();
        current.coverage = Outcome::Truncated;
        let mut changed = Reads::fixture(BTreeMap::from([(root.join("online"), b"0".to_vec())]));
        validate_mapping(&mut document, &root, &mut changed, &mut current);
        assert_eq!(current.coverage, Outcome::Truncated);
        assert!(!current.verified_mapping);
        assert!(
            current
                .rows
                .iter()
                .all(|row| row.values.is_none() && row.outcome == Outcome::Unsupported)
        );
        current.retain(&previous);
        assert!(current.rows.iter().all(|row| row.values.is_none()));
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "NUMA distance mapping validation"
                    && fact.outcome == Outcome::Unsupported)
        );
    }

    #[test]
    fn sparse_node_ids_preserve_distance_order_asymmetry_cpu_and_memory_data() {
        let (_temp, root, mut reads, mut doc) = fixture();
        let mut pending = false;
        collect_at(&mut doc, &root, &root, &mut reads, &mut |doc| {
            if let Some(data) = &doc.numa {
                pending = true;
                if data.coverage == Outcome::NotRequested {
                    assert!(data.rows.iter().all(|row| row.values.is_none()));
                }
            }
        });
        let data = doc.numa.as_ref().unwrap();
        assert_eq!(data.columns, vec![0, 7]);
        assert_eq!(data.rows.len(), 2);
        assert_eq!(data.rows[0].values, Some(vec![10, 32]));
        assert_eq!(data.rows[1].values, Some(vec![40, 10]));
        assert!(pending);
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "node7 Total memory"
                    && fact.semantic == Some(Value::Integer(1048576)))
        );
        assert!(
            doc.facts
                .iter()
                .any(|fact| fact.label == "node0 distance column mapping"
                    && fact.value.as_ref().unwrap().contains("node7: 32"))
        );
        assert!(reads.attempts().all(|path| !matches!(
            path.file_name().and_then(|n| n.to_str()),
            Some("compact" | "reclaim" | "state" | "numastat" | "vmstat")
        )));
    }
    #[test]
    fn retained_sources_do_not_become_fresh_memory_or_distance_values() {
        let (_temp, root, mut reads, mut doc) = fixture();
        let node = root.join("node7");
        for (name, raw) in [
            ("cpulist", "8-11"),
            (
                "meminfo",
                "Node 7 MemTotal: 1024 kB\nNode 7 MemFree: 256 kB",
            ),
            ("distance", "40 10"),
        ] {
            let mut fact = Fact::text(name, raw, node.join(name));
            fact.retained = true;
            fact.outcome = Outcome::PermissionDenied;
            fact.observed_ms = 42;
            doc.facts.push(fact);
        }
        collect_at(&mut doc, &node, &root, &mut reads, &mut |_| {});
        let row = &doc.numa.as_ref().unwrap().rows[0];
        assert!(row.values.is_none());
        assert_eq!(row.outcome, Outcome::PermissionDenied);
        assert_eq!(row.observed_ms, 42);
        assert!(!doc.numa.as_ref().unwrap().summary.contains("CPUs"));
        assert!(
            !doc.facts
                .iter()
                .any(|fact| fact.label == "node7 Total memory")
        );
    }

    #[test]
    fn decoded_memory_preserves_source_observation_time_and_generation() {
        let (_temp, root, _, mut doc) = fixture();
        let node = root.join("node7");
        doc.facts.clear();
        let mut source = Fact::text(
            "NUMA memory information",
            "Node 7 MemTotal: 1024 kB\nNode 7 MemFree: 256 kB",
            node.join("meminfo"),
        );
        source.observed_ms = 42;
        source.observed_generation = Some(7);
        doc.facts.push(source);
        node_facts(&mut doc, &node, 7, true, &mut String::new());
        for fact in doc
            .facts
            .iter()
            .filter(|fact| fact.label == "node7 Total memory" || fact.label == "node7 Free memory")
        {
            assert_eq!(fact.observed_ms, 42);
            assert_eq!(fact.observed_generation, Some(7));
            assert_eq!(fact.source, node.join("meminfo"));
        }
        assert_eq!(
            doc.facts
                .iter()
                .filter(|fact| fact.origin.starts_with("decoded node meminfo"))
                .count(),
            2
        );
    }

    #[test]
    fn decoded_legacy_memory_does_not_invent_source_generation() {
        let (_temp, root, _, mut doc) = fixture();
        let node = root.join("node7");
        let source = Fact::text(
            "NUMA memory information",
            "Node 7 MemTotal: 1024 kB\nNode 7 MemFree: 256 kB",
            node.join("meminfo"),
        );
        let mut legacy = serde_json::to_value(source).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("observed_generation");
        legacy.as_object_mut().unwrap().remove("generation_pending");
        doc.facts = vec![serde_json::from_value(legacy).unwrap()];
        node_facts(&mut doc, &node, 7, true, &mut String::new());
        doc.generation = 99;
        doc.stamp_fact_generations();
        assert!(
            doc.facts
                .iter()
                .filter(|fact| fact.origin.starts_with("decoded node meminfo"))
                .all(|fact| fact.observed_generation.is_none())
        );
    }

    #[test]
    fn derived_distance_mapping_preserves_source_generation_and_time() {
        let (_temp, root, mut reads, mut doc) = fixture();
        let node = root.join("node7");
        let mut source = Fact::text("NUMA relative distances", "40 10", node.join("distance"));
        source.observed_ms = 42;
        source.observed_generation = Some(7);
        doc.facts.push(source);
        collect_at(&mut doc, &node, &root, &mut reads, &mut |_| {});
        let derived = doc
            .facts
            .iter()
            .find(|fact| fact.label == "node7 distance column mapping")
            .unwrap();
        assert_eq!(derived.observed_ms, 42);
        assert_eq!(derived.observed_generation, Some(7));
        assert_eq!(derived.source, node.join("distance"));
    }

    #[test]
    fn malformed_or_large_data_never_invents_memory_or_distance_relationships() {
        assert_eq!(ids("0,7").unwrap(), vec![0, 7]);
        assert!(ids("0-1000000").is_err());
        assert!(distances("10", &[0, 7]).is_err());
        assert!(distances("10 unknown", &[0, 7]).is_err());
        for raw in [
            "Node 0 MemTotal: 18446744073709551615 kB",
            "Node 0 MemTotal: 4 MB",
            "Node 1 MemTotal: 4 kB",
            "Node 0 MemTotal: 4 kB\nNode 0 MemTotal: 8 kB",
        ] {
            assert!(memory(raw, 0, "MemTotal").is_err());
        }
        assert!(rules(Path::new("/unregistered/node0")).is_empty());
    }
    #[test]
    fn changed_online_set_suppresses_mapping_and_retention_requires_matching_columns() {
        let (_temp, root, mut reads, mut doc) = fixture();
        let old = Numa {
            summary: "old".into(),
            columns: vec![0],
            rows: vec![Row {
                id: 0,
                values: Some(vec![10]),
                observed_ms: 42,
                retained: false,
                outcome: Outcome::Observed,
            }],
            coverage: Outcome::Observed,
            verified_mapping: true,
        };
        let mut changed = old.clone();
        validate_mapping(&mut doc, &root, &mut reads, &mut changed);
        assert!(changed.rows[0].values.is_none());
        assert_eq!(changed.coverage, Outcome::Unsupported);
        changed.verified_mapping = false;
        changed.retain(&old);
        assert!(changed.rows[0].values.is_none());
        changed.verified_mapping = true;
        changed.retain(&old);
        assert!(changed.rows[0].retained);
        assert_eq!(changed.rows[0].observed_ms, 42);
        let mut different = changed;
        different.rows[0].values = None;
        different.columns = vec![7];
        different.retain(&old);
        assert!(different.rows[0].values.is_none());
    }
    #[test]
    fn single_node_preview_excludes_unknown_nested_directories_and_keeps_cpu_memory_links() {
        let (temp, root, mut reads, mut doc) = fixture();
        let node = root.join("node0");
        let cpu = temp.path().join("cpu0");
        let memory = temp.path().join("memory0");
        std::fs::create_dir(&cpu).unwrap();
        std::fs::create_dir(&memory).unwrap();
        symlink(&cpu, node.join("cpu0")).unwrap();
        symlink(&memory, node.join("memory0")).unwrap();
        collect_at(&mut doc, &node, &root, &mut reads, &mut |_| {});
        assert!(doc.numa.as_ref().unwrap().summary.contains("CPUs 0-3"));
        assert!(doc.relations.iter().any(|relation| relation.target == cpu));
        assert!(
            doc.relations
                .iter()
                .any(|relation| relation.target == memory)
        );
        let private = node.join("private");
        std::fs::create_dir(&private).unwrap();
        assert!(rules(&private).is_empty());
    }
}

#[cfg(test)]
mod rendering_tests {
    use super::*;
    #[test]
    fn distance_cells_stay_inside_compact_rows_in_both_appearances() {
        use little_clip_core::{config::WorkbenchAppearance, palette::AppPalette};
        let data = Numa {
            summary: "node0 and node7".into(),
            columns: vec![0, 7],
            rows: vec![
                Row {
                    id: 0,
                    values: Some(vec![10, 32]),
                    observed_ms: 0,
                    retained: false,
                    outcome: Outcome::Observed,
                },
                Row {
                    id: 7,
                    values: Some(vec![40, 10]),
                    observed_ms: 0,
                    retained: true,
                    outcome: Outcome::TimedOut,
                },
            ],
            coverage: Outcome::Observed,
            verified_mapping: true,
        };
        let rect = egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(600.0, 42.0));
        for appearance in [WorkbenchAppearance::Dark, WorkbenchAppearance::Light] {
            let palette = AppPalette::for_appearance(appearance);
            let colors = crate::ui_appearance::WorkbenchColors::from_palette(&palette);
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
                        paint(ui, rect, &data, colors, &palette);
                    });
                },
            );
            let cells = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Rect(shape) if rect.contains_rect(shape.rect) => {
                        Some(shape.rect)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(cells.len(), 5);
            assert!(
                cells
                    .iter()
                    .all(|cell| cell.width() > 0.0 && cell.height() > 0.0)
            );
            assert!(output.shapes.iter().any(|shape|matches!(&shape.shape,egui::epaint::Shape::Text(text) if text.galley.job.text=="N7")));
        }
    }
}
