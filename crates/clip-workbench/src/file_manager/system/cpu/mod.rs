//! Passive CPU topology, cache sharing and policy observations from sysfs.
#[cfg(target_arch = "x86_64")]
mod identification;
mod maps;
mod ui;
use super::{
    Document, Relation,
    attributes::{self, Context, Family},
    collector::Reads,
    model::{Fact, Outcome, Value},
};
use serde::{Deserialize, Serialize};
use std::{io, path::Path};
pub(super) use ui::{inspector, paint};
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(in crate::file_manager) struct Cpu {
    pub role: String,
    pub summary: String,
    pub bins: Vec<maps::Bin>,
    pub memberships: Vec<String>,
    pub focus: Option<usize>,
    pub coverage: Outcome,
    pub retained: bool,
}
struct Member {
    label: String,
    ranges: Option<Vec<(u32, u32)>>,
}
fn numbered(name: &str, prefix: &str) -> Option<u32> {
    name.strip_prefix(prefix)?.parse().ok()
}

pub(super) fn directory_group(
    sys: &Path,
    kind: little_clip_core::kernel_interface::Kind,
) -> Option<(Option<std::path::PathBuf>, String)> {
    use little_clip_core::kernel_interface::{Filesystem, Kind};
    let name = sys.file_name()?.to_str()?;
    if Context::discover(sys).family != Family::Cpu || !super::buses::numbered(name, "cpu") {
        return None;
    }
    let number = numbered(name, "cpu")?;
    let role = match kind {
        Kind::Character { major: 202, minor } if minor == number => "MSR interface (unopened)",
        Kind::Character { major: 203, minor } if minor == number => "CPUID interface (unopened)",
        Kind::Kernel {
            filesystem: Filesystem::DeviceNodes,
        } => "Logical CPU directory",
        _ => return None,
    };
    Some((Some(sys.into()), role.into()))
}
fn observe(document: &mut Document, reads: &mut Reads, path: &Path, name: &str) -> Option<Fact> {
    let mut context = Context::from_parts(path, Some("cpu"));
    if path.file_name().is_some_and(|name| name == "topology") {
        context.relative = "topology".into();
    }
    let rule = attributes::rule(&context, name)?;
    Some(reads.observe(document, path, &rule))
}
fn membership(document: &mut Document, reads: &mut Reads, path: &Path, name: &str) -> Member {
    let fact = observe(document, reads, path, name);
    let label = fact
        .as_ref()
        .map_or_else(|| name.into(), |fact| fact.label.clone());
    let ranges = fact.and_then(|fact| match fact.semantic {
        Some(Value::CpuList(ranges)) if fact.outcome == Outcome::Observed => Some(ranges),
        _ => None,
    });
    Member { label, ranges }
}
fn root_sets(document: &mut Document, reads: &mut Reads, root: &Path) -> Vec<Member> {
    [
        "online",
        "present",
        "possible",
        "offline",
        "isolated",
        "nohz_full",
    ]
    .map(|name| membership(document, reads, root, name))
    .into()
}
pub(super) fn collect(
    document: &mut Document,
    sys: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let Some(root) = sys
        .ancestors()
        .find(|path| path.file_name().is_some_and(|name| name == "cpu"))
    else {
        return;
    };
    if Context::discover(sys).family != Family::Cpu {
        return;
    }
    let logical = sys.ancestors().find_map(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| numbered(name, "cpu"))
            .map(|id| (id, path))
    });
    let mut members = root_sets(document, reads, root);
    let leaf = sys.file_name().unwrap_or_default().to_string_lossy();
    let role = role(sys, logical.is_some());
    let summary = match role {
        "CPU cache" => {
            for rule in attributes::rules(&Context::from_parts(sys, Some("cpu"))) {
                observe(document, reads, sys, &rule.name);
            }
            members.push(membership(document, reads, sys, "shared_cpu_list"));
            format!(
                "L{} · {} · {}",
                value(document, "Cache level"),
                value(document, "Cache type"),
                value(document, "Cache size")
            )
        }
        "CPU frequency policy" => {
            frequency(document, reads, sys);
            members.push(membership(document, reads, sys, "related_cpus"));
            format!(
                "{} · {} · {}",
                leaf,
                value(document, "Governor"),
                value(document, "Requested/current policy frequency")
            )
        }
        _ => match logical {
            Some((id, cpu)) => logical_cpu(document, reads, cpu, id, &mut members, checkpoint),
            None => {
                inventory(document, sys);
                "CPU membership".into()
            }
        },
    };
    let requested = document
        .facts
        .iter()
        .find(|fact| {
            fact.source == document.path && matches!(fact.semantic, Some(Value::CpuList(_)))
        })
        .map(|fact| fact.label.clone());
    let requested_focus = requested.as_ref().map(|label| {
        if let Some(index) = members.iter().position(|member| member.label == *label) {
            index
        } else {
            let name = document
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            members.push(membership(document, reads, sys, &name));
            members.len() - 1
        }
    });
    // Include every observed mask, including reserved/offline IDs beyond the
    // possible set and useful sets whose other domain attributes were denied.
    let domain = attributes::merge_cpu_ranges(
        members
            .iter()
            .filter_map(|member| member.ranges.as_ref())
            .flat_map(|ranges| ranges.iter().copied())
            .collect(),
    );
    let bins = maps::bins(&domain, &members, logical.map(|(id, _)| id));
    let focus = requested_focus.or_else(|| (members.len() > 6).then_some(members.len() - 1));
    let coverage = document
        .facts
        .iter()
        .filter(|fact| fact.source.starts_with(root))
        .find(|fact| !matches!(fact.outcome, Outcome::Observed | Outcome::Missing))
        .map_or_else(
            || {
                if bins.is_empty() {
                    Outcome::Missing
                } else {
                    Outcome::Observed
                }
            },
            |fact| fact.outcome.clone(),
        );
    let mut data = Cpu {
        role: role.into(),
        summary,
        bins,
        memberships: members.into_iter().map(|member| member.label).collect(),
        focus,
        coverage,
        retained: false,
    };
    if logical.is_none() && role == "CPU topology" {
        data.summary = format!(
            "{} online · {} present · {} possible logical CPUs",
            count_label(data.count(0)),
            count_label(data.count(1)),
            count_label(data.count(2))
        );
    }
    if let Some(index) = requested_focus {
        data.summary = format!(
            "{} · {} reported CPU IDs",
            data.memberships[index],
            count_label(data.count(index))
        );
    }
    document.cpu = Some(data);
    if document.interface_directory
        && matches!(
            document.kind,
            little_clip_core::kernel_interface::Kind::Kernel {
                filesystem: little_clip_core::kernel_interface::Filesystem::Sysfs
            }
        )
    {
        document.title = role.into();
    }
    document.facts.push(Fact::text("CPU interpretation","Topology IDs and sibling sets are kernel-reported and can have architecture defaults. Policy frequency can be a requested P-state; it is not a guaranteed instantaneous hardware clock. No CPUID or MSR device is opened.",sys.into()));
    checkpoint(document);
}
fn count_label(count: Option<u32>) -> String {
    count.map_or_else(|| "unavailable".into(), |count| count.to_string())
}
fn role(sys: &Path, logical: bool) -> &'static str {
    let leaf = sys.file_name().unwrap_or_default().to_string_lossy();
    if numbered(&leaf, "index").is_some()
        && sys
            .parent()
            .is_some_and(|path| path.file_name().is_some_and(|name| name == "cache"))
    {
        "CPU cache"
    } else if numbered(&leaf, "policy").is_some() {
        "CPU frequency policy"
    } else if logical {
        "Logical CPU topology"
    } else {
        "CPU topology"
    }
}
fn value<'a>(document: &'a Document, label: &str) -> &'a str {
    document
        .facts
        .iter()
        .find(|fact| fact.label == label && fact.outcome == Outcome::Observed)
        .and_then(|fact| fact.value.as_deref())
        .unwrap_or("unavailable")
}
fn logical_cpu(
    document: &mut Document,
    reads: &mut Reads,
    cpu: &Path,
    id: u32,
    members: &mut Vec<Member>,
    checkpoint: &mut dyn FnMut(&Document),
) -> String {
    #[cfg(target_arch = "x86_64")]
    identification::collect(document, cpu, id);
    let topology = cpu.join("topology");
    for name in [
        "physical_package_id",
        "core_id",
        "die_id",
        "cluster_id",
        "core_cpus_list",
        "package_cpus_list",
    ] {
        observe(document, reads, &topology, name);
    }
    let mut smt = membership(document, reads, &topology, "core_cpus_list");
    if smt.ranges.is_none() {
        smt = membership(document, reads, &topology, "thread_siblings_list");
    }
    members.push(smt);
    document
        .relations
        .push(Relation::new("CPU topology", topology));
    let policy = cpu.join("cpufreq");
    if let Ok(target) = policy.canonicalize() {
        document
            .relations
            .push(Relation::new("Frequency policy", target));
        frequency(document, reads, &policy);
    }
    checkpoint(document);
    caches(document, reads, &cpu.join("cache"), checkpoint);
    inventory(document, cpu);
    let status = members[0]
        .ranges
        .as_ref()
        .map_or("online status unavailable", |ranges| {
            if ranges
                .iter()
                .any(|(first, last)| (*first..=*last).contains(&id))
            {
                "Online"
            } else {
                "Offline in observed online set"
            }
        });
    format!(
        "cpu{id} · Package {} · Core {} · {status} · {}",
        value(document, "Package"),
        value(document, "Core"),
        value(document, "Requested/current policy frequency")
    )
}
fn frequency(document: &mut Document, reads: &mut Reads, policy: &Path) {
    for name in [
        "scaling_governor",
        "scaling_cur_freq",
        "cpuinfo_cur_freq",
        "scaling_min_freq",
        "scaling_max_freq",
        "cpuinfo_min_freq",
        "cpuinfo_max_freq",
        "scaling_driver",
        "related_cpus",
        "affected_cpus",
    ] {
        observe(document, reads, policy, name);
    }
}
fn caches(
    document: &mut Document,
    reads: &mut Reads,
    cache: &Path,
    checkpoint: &mut dyn FnMut(&Document),
) {
    let entries = match std::fs::read_dir(cache) {
        Ok(entries) => entries,
        Err(error) => {
            document
                .facts
                .push(Fact::failure("Cache inventory", cache.into(), error));
            return;
        }
    };
    let (paths, coverage) = super::buses::select_children(
        entries,
        |entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| numbered(name, "index").is_some())
        },
        8,
    );
    if coverage != Outcome::Observed {
        document.facts.push(Fact::failure(
            "Cache inventory",
            cache.into(),
            io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Cache inventory incomplete or capped",
            ),
        ));
    }
    for path in paths {
        document
            .relations
            .push(Relation::new("CPU cache", path.clone()));
        let mut parts = Vec::new();
        for rule in attributes::rules(&Context::from_parts(&path, Some("cpu"))) {
            let fact = reads.fact(&path, &rule);
            if let Some(value) = &fact.value {
                parts.push(format!("{}: {value}", fact.label));
            }
            if !matches!(fact.outcome, Outcome::Observed | Outcome::Missing) {
                document.facts.push(fact);
            }
        }
        document.facts.push(Fact::text(
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            parts.join(" · "),
            path,
        ));
        checkpoint(document);
    }
}
fn inventory(document: &mut Document, root: &Path) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            document.facts.push(Fact::failure(
                "CPU relationship inventory",
                root.into(),
                error,
            ));
            return;
        }
    };
    for (index, entry) in entries.enumerate().take(257) {
        if index == 256 {
            document.facts.push(Fact::failure(
                "CPU relationship inventory",
                root.into(),
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "CPU inventory capped at 256 entries",
                ),
            ));
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                document.facts.push(Fact::failure(
                    "CPU relationship inventory",
                    root.into(),
                    error,
                ));
                continue;
            }
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let label = if numbered(&name, "cpu").is_some() {
            "Logical CPU"
        } else if numbered(&name, "node").is_some() {
            "NUMA node"
        } else if numbered(&name, "policy").is_some() {
            "Frequency policy"
        } else if name == "cpufreq" {
            "Frequency policies"
        } else {
            continue;
        };
        let target = match entry.path().canonicalize() {
            Ok(target) => target,
            Err(error) => {
                document
                    .facts
                    .push(Fact::failure(label, entry.path(), error));
                continue;
            }
        };
        if document.relations.len() >= 96 {
            document.facts.push(Fact::failure(
                "CPU relationship inventory",
                root.into(),
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "CPU relationship display capped at 96 links",
                ),
            ));
            break;
        }
        document.relations.push(Relation::new(label, target));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    fn fixture() -> (tempfile::TempDir, Document, Reads) {
        let root = tempfile::tempdir().unwrap();
        let cpu = root.path().join("cpu/cpu3");
        std::fs::create_dir_all(cpu.join("topology")).unwrap();
        std::fs::create_dir_all(cpu.join("cpufreq")).unwrap();
        let subsystem = root.path().join("bus/cpu");
        std::fs::create_dir_all(&subsystem).unwrap();
        std::os::unix::fs::symlink(&subsystem, cpu.join("subsystem")).unwrap();
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        document.path = cpu.clone();
        document.interface_directory = true;
        document.kind = little_clip_core::kernel_interface::Kind::Kernel {
            filesystem: little_clip_core::kernel_interface::Filesystem::Sysfs,
        };
        document.facts.clear();
        let mut values = BTreeMap::new();
        for (name, value) in [
            ("online", "3"),
            ("possible", "3,9"),
            ("present", "3,9"),
            ("offline", "9"),
            ("isolated", ""),
            ("nohz_full", ""),
            ("cpu3/topology/physical_package_id", "0"),
            ("cpu3/topology/core_id", "7"),
            ("cpu3/topology/core_cpus_list", "3,9"),
            ("cpu3/topology/package_cpus_list", "3,9"),
            ("cpu3/cpufreq/scaling_cur_freq", "4200000"),
            ("cpu3/cpufreq/scaling_governor", "schedutil"),
            ("cpu3/cpufreq/related_cpus", "3 9"),
            ("cpu3/cpufreq/affected_cpus", "3"),
        ] {
            values.insert(
                root.path().join("cpu").join(name),
                value.as_bytes().to_vec(),
            );
        }
        (root, document, Reads::fixture(values))
    }
    #[test]
    fn logical_cpu_uses_root_online_set_and_policy_without_low_level_device_reads() {
        let (_root, mut document, mut reads) = fixture();
        let path = document.path.clone();
        collect(&mut document, &path, &mut reads, &mut |_| {});
        assert_eq!(document.title, "Logical CPU topology");
        let data = document.cpu.unwrap();
        assert!(
            data.summary
                .contains("cpu3 · Package 0 · Core 7 · Online · 4.2 GHz")
        );
        assert_eq!(data.bins.len(), 2);
        assert!(data.bins[0].selected);
        assert_eq!(data.count(0), Some(1));
        assert_eq!(data.count(6), Some(2));
        assert!(
            reads
                .attempts()
                .all(|path| !path.to_string_lossy().contains("/dev/")
                    && !path.ends_with("msr")
                    && !path.ends_with("cpuid"))
        );
    }
    #[test]
    fn whitespace_policy_cpu_lists_are_typed_and_missing_masks_stay_unknown() {
        let (_root, mut document, mut reads) = fixture();
        let path = document.path.join("cpufreq");
        let member = membership(&mut document, &mut reads, &path, "related_cpus");
        assert_eq!(member.ranges, Some(vec![(3, 3), (9, 9)]));
        let missing = membership(&mut document, &mut reads, &path, "possible");
        assert!(missing.ranges.is_none());
    }
    #[test]
    fn masks_outside_the_possible_domain_and_unknown_memberships_are_preserved() {
        let (_root, mut document, mut reads) = fixture();
        let root = document.path.parent().unwrap().to_path_buf();
        let mut offline = Fact::text("Offline CPUs", "1000", root.join("offline"));
        offline.semantic = Some(Value::CpuList(vec![(1000, 1000)]));
        document.facts.push(offline);
        document.facts.push(Fact::failure(
            "Isolated CPUs",
            root.join("isolated"),
            io::Error::from(io::ErrorKind::PermissionDenied),
        ));
        let path = document.path.clone();
        collect(&mut document, &path, &mut reads, &mut |_| {});
        let data = document.cpu.unwrap();
        assert_eq!(
            data.bins.iter().map(|bin| bin.first).collect::<Vec<_>>(),
            vec![3, 9, 1000]
        );
        assert_eq!(data.bins[2].counts[2], Some(0));
        assert_eq!(data.bins[2].counts[3], Some(1));
        assert_eq!(data.bins[2].counts[4], None);
        assert_eq!(data.coverage, Outcome::PermissionDenied);
    }
    #[test]
    fn cache_directories_and_policy_roles_use_specific_metadata() {
        assert_eq!(
            role(Path::new("/sys/devices/system/cpu/cpu3/cache/index0"), true),
            "CPU cache"
        );
        assert_eq!(
            role(Path::new("/sys/devices/system/cpu/cpufreq/policy3"), false),
            "CPU frequency policy"
        );
        assert_eq!(
            role(Path::new("/sys/devices/system/cpu/cpufreq"), false),
            "CPU topology"
        );
    }
}
