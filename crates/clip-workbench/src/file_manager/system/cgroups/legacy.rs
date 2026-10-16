//! Documented v1 controller getters; never migration, reclaim, freezing or reset writes.
use super::{Document, Fact, Format, Reads, Rule, Value, attributes, invalid, unsigned};
use std::{collections::BTreeSet, io, os::unix::fs::MetadataExt, path::Path};

#[derive(Clone, Copy)]
pub(super) enum Parser {
    Quota,
    Period,
    MemoryLimit,
    Nanoseconds,
    BandwidthStats,
    AccountingStats,
}
use super::Parser::{CountLimit, Legacy, Members, Standard};
pub(super) const FIELDS: &[(&str, &str, super::Parser)] = &[
    (
        "cpu.shares",
        "Local CPU scheduling shares",
        Standard(Format::Unsigned),
    ),
    (
        "cpu.cfs_quota_us",
        "Local fair-class CPU quota",
        Legacy(Parser::Quota),
    ),
    (
        "cpu.cfs_period_us",
        "Local fair-class CPU period",
        Legacy(Parser::Period),
    ),
    (
        "cpu.cfs_burst_us",
        "Configured CPU burst allowance",
        Standard(Format::UnsignedScale(1.0, "µs")),
    ),
    (
        "cpu.stat",
        "Accumulated CPU bandwidth accounting",
        Legacy(Parser::BandwidthStats),
    ),
    (
        "cpuacct.usage",
        "Total accumulated CPU time including descendants",
        Legacy(Parser::Nanoseconds),
    ),
    (
        "cpuacct.usage_user",
        "Accumulated user CPU time including descendants",
        Legacy(Parser::Nanoseconds),
    ),
    (
        "cpuacct.usage_sys",
        "Accumulated system CPU time including descendants",
        Legacy(Parser::Nanoseconds),
    ),
    (
        "cpuacct.stat",
        "Accumulated CPU mode accounting",
        Legacy(Parser::AccountingStats),
    ),
    (
        "cpuset.cpus",
        "Configured CPU membership",
        Standard(Format::CpuList),
    ),
    (
        "cpuset.effective_cpus",
        "Effective CPU membership",
        Standard(Format::CpuList),
    ),
    (
        "cpuset.mems",
        "Configured NUMA membership",
        Standard(Format::CpuList),
    ),
    (
        "cpuset.effective_mems",
        "Effective NUMA membership",
        Standard(Format::CpuList),
    ),
    (
        "memory.usage_in_bytes",
        "Approximate memory accounting",
        Standard(Format::Bytes),
    ),
    (
        "memory.limit_in_bytes",
        "Local memory limit setting",
        Legacy(Parser::MemoryLimit),
    ),
    (
        "memory.memsw.usage_in_bytes",
        "Approximate combined memory and swap accounting",
        Standard(Format::Bytes),
    ),
    (
        "memory.memsw.limit_in_bytes",
        "Local combined memory and swap limit setting",
        Legacy(Parser::MemoryLimit),
    ),
    (
        "memory.max_usage_in_bytes",
        "Recorded peak memory accounting",
        Standard(Format::Bytes),
    ),
    (
        "memory.failcnt",
        "Memory limit-hit counter (resettable)",
        Standard(Format::Unsigned),
    ),
    (
        "memory.use_hierarchy",
        "Legacy reported hierarchical memory-accounting flag",
        Standard(Format::Boolean("Enabled", "Disabled")),
    ),
    (
        "memory.swappiness",
        "Configured reclaim swappiness",
        Standard(Format::Unsigned),
    ),
    (
        "pids.current",
        "Current task count including descendants",
        Standard(Format::Unsigned),
    ),
    ("pids.max", "Local task limit", CountLimit),
    ("cgroup.procs", "Observed process membership", Members),
    ("tasks", "Observed thread membership", Members),
];
const BANDWIDTH: &[(&str, &str, &str)] = &[
    ("throttled_time", "Accumulated throttling time", " ns"),
    ("burst_time", "Accumulated burst time", " ns"),
];
const ACCOUNTING: &[(&str, &str, &str)] = &[
    ("user", "Accumulated user CPU time", " USER_HZ ticks"),
    ("system", "Accumulated system CPU time", " USER_HZ ticks"),
];
pub(super) fn parse(parser: Parser, name: &str, raw: &str) -> io::Result<(Value, String)> {
    match parser {
        Parser::Quota => {
            let quota = raw.parse::<i64>().map_err(|_| invalid())?;
            let text = if quota == -1 {
                "No local bandwidth limit (ancestor constraints may apply)".into()
            } else if quota > 0 {
                format!("{quota} µs per configured period (local fair-class bandwidth)")
            } else {
                return Err(invalid());
            };
            Ok((Value::Integer(quota.into()), text))
        }
        Parser::Period => {
            let value = unsigned(raw)?;
            if value == 0 {
                return Err(invalid());
            }
            Ok((Value::Integer(value.into()), format!("{value} µs")))
        }
        Parser::MemoryLimit => {
            let (value, text) = attributes::parse(&Rule::new(name, name, Format::Bytes), raw)?;
            Ok((
                value,
                format!(
                    "{text} · local setting; very large values may represent the kernel no-limit sentinel; ancestor constraints may apply"
                ),
            ))
        }
        Parser::Nanoseconds => Ok((
            Value::Integer(unsigned(raw)?.into()),
            format!("{raw} ns · accumulated time, not utilization"),
        )),
        Parser::BandwidthStats => super::cpu_stats(raw, BANDWIDTH),
        Parser::AccountingStats => super::cpu_stats(raw, ACCOUNTING),
    }
}
const CONTROLLERS: &[&str] = &[
    "cpu",
    "cpuacct",
    "cpuset",
    "memory",
    "pids",
    "blkio",
    "devices",
    "freezer",
    "hugetlb",
    "net_cls",
    "net_prio",
    "perf_event",
    "rdma",
    "misc",
    "debug",
];
pub(super) fn controllers(document: &mut Document, root: &Path, reads: &mut Reads) {
    let result = (|| {
        let device = std::fs::metadata(root)?.dev();
        let inventory = super::super::storage::mounts::parse(&reads.mountinfo()?);
        let mut names = BTreeSet::new();
        let mut options = BTreeSet::new();
        let mut mounts = BTreeSet::new();
        for mount in inventory.records.iter().filter(|mount| {
            mount.filesystem == "cgroup"
                && mount.device == (libc::major(device), libc::minor(device))
        }) {
            mounts.insert(mount.id);
            options.extend(mount.super_options.iter().cloned());
            names.extend(
                mount
                    .super_options
                    .iter()
                    .filter(|name| {
                        CONTROLLERS.contains(&name.as_str()) || name.starts_with("name=")
                    })
                    .cloned(),
            );
        }
        if mounts.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "No matching legacy mount metadata in this mount namespace",
            ));
        }
        let mut fact = Fact::text(
            "Legacy hierarchy controllers",
            if names.is_empty() {
                "No recognized controllers reported by matching mount metadata".into()
            } else {
                names.into_iter().collect::<Vec<_>>().join(", ")
            },
            "/proc/self/mountinfo".into(),
        );
        let truncated = options.len() > 32;
        let mut metadata = Fact::text(
            "Legacy hierarchy mount options",
            options
                .iter()
                .take(32)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            "/proc/self/mountinfo".into(),
        );
        if truncated {
            metadata
                .value
                .as_mut()
                .unwrap()
                .push_str(" · first 32 reported options shown");
        }
        metadata.outcome = if truncated {
            super::Outcome::Truncated
        } else {
            inventory.outcome.clone()
        };
        document.facts.push(metadata);
        fact.outcome = inventory.outcome;
        Ok(fact)
    })();
    document.facts.push(result.unwrap_or_else(|error| {
        let mut fact = Fact::failure(
            "Legacy hierarchy controllers",
            "/proc/self/mountinfo".into(),
            error,
        );
        fact.value = Some("Controller mapping unavailable in this mount view".into());
        fact
    }));
}

#[cfg(test)]
mod tests {
    use super::super::{Outcome, Version, collect_version};
    use super::*;
    use std::collections::BTreeMap;
    #[test]
    fn legacy_units_and_no_limit_semantics_do_not_use_v2_interpretations() {
        assert!(
            parse(Parser::Quota, "cpu.cfs_quota_us", "-1")
                .unwrap()
                .1
                .contains("ancestor")
        );
        assert!(
            parse(Parser::Quota, "cpu.cfs_quota_us", "50000")
                .unwrap()
                .1
                .contains("µs")
        );
        for raw in ["-2", "0", "max"] {
            assert!(parse(Parser::Quota, "cpu.cfs_quota_us", raw).is_err());
        }
        assert!(parse(Parser::Period, "cpu.cfs_period_us", "0").is_err());
        let text = parse(
            Parser::BandwidthStats,
            "cpu.stat",
            "throttled_time 123\nnr_periods 2\nfuture 9",
        )
        .unwrap()
        .1;
        assert!(text.contains("123 ns") && text.contains("future: 9") && !text.contains("µs"));
        assert!(
            parse(Parser::AccountingStats, "cpuacct.stat", "user 5\nsystem 7")
                .unwrap()
                .1
                .contains("USER_HZ ticks")
        );
        let limit = parse(
            Parser::MemoryLimit,
            "memory.limit_in_bytes",
            "9223372036854771712",
        )
        .unwrap()
        .1;
        assert!(limit.contains("may represent") && !limit.contains("No local limit"));
    }
    #[test]
    fn legacy_collection_keeps_selected_getters_and_excludes_controls() {
        let (_temp, root, mut reads, mut document) =
            super::super::super::collector::registered_fixture(
                "fixture",
                "group",
                &[
                    ("cpu.cfs_quota_us", "-1"),
                    ("cpu.cfs_period_us", "100000"),
                    ("cpu.stat", "throttled_time 100"),
                    ("memory.usage_in_bytes", "4096"),
                    ("tasks", "8\n9"),
                    ("memory.force_empty", "forbidden"),
                ],
            );
        document.path = root.clone();
        document.interface_directory = true;
        collect_version(
            &mut document,
            &root,
            &mut reads,
            &mut |_| {},
            Version::Legacy,
        );
        assert_eq!(
            document.observed_raw("Local fair-class CPU quota"),
            Some("-1")
        );
        assert!(
            document
                .facts
                .iter()
                .any(|f| f.label == "Accumulated CPU bandwidth accounting"
                    && f.value.as_deref().unwrap_or("").contains("100 ns"))
        );
        assert!(
            !reads
                .attempts()
                .any(|path| path.ends_with("memory.force_empty") || path.ends_with("cpu.max"))
        );
        document.path = root.join("tasks");
        document.interface_directory = false;
        document.facts.clear();
        collect_version(
            &mut document,
            &root,
            &mut reads,
            &mut |_| {},
            Version::Legacy,
        );
        assert_eq!(
            document.observed_raw("Observed thread membership"),
            Some("8\n9")
        );
        assert!(
            !document
                .facts
                .iter()
                .any(|f| f.label == "Local fair-class CPU quota")
        );
        document.path = root.join("memory.force_empty");
        document.facts.clear();
        collect_version(
            &mut document,
            &root,
            &mut reads,
            &mut |_| {},
            Version::Legacy,
        );
        assert!(document.facts.iter().any(|f| {
            f.value
                .as_deref()
                .is_some_and(|text| text.contains("payload not read"))
        }));
    }
    #[test]
    fn controllers_follow_matching_mount_identity_not_path_prefix() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let device = std::fs::metadata(root).unwrap().dev();
        let raw = format!(
            "30 1 {}:{} / /different/alias rw - cgroup cgroup rw,cpu,cpuacct,name=demo,futurectrl\n31 1 8:1 / /same/path rw - cgroup cgroup rw,memory\n",
            libc::major(device),
            libc::minor(device)
        );
        let mut reads = Reads::fixture(BTreeMap::from([(
            "/proc/self/mountinfo".into(),
            raw.into_bytes(),
        )]));
        let mut document =
            super::super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        controllers(&mut document, root, &mut reads);
        let fact = document.facts.last().unwrap();
        assert_eq!(fact.outcome, Outcome::Observed);
        assert_eq!(fact.value.as_deref(), Some("cpu, cpuacct, name=demo"));
        assert!(document.facts.iter().any(|fact| {
            fact.label == "Legacy hierarchy mount options"
                && fact
                    .value
                    .as_deref()
                    .is_some_and(|value| value.contains("futurectrl"))
        }));
    }
    #[test]
    fn legacy_numa_ranges_and_unknown_counter_keys_keep_their_meaning() {
        let (_, display) =
            super::super::parse(Standard(Format::CpuList), "cpuset.effective_mems", "0-2,8")
                .unwrap();
        assert!(display.contains("4 NUMA nodes") && !display.contains("logical CPUs"));
        for raw in ["user 1\nuser 2", "system -1", "user 1 extra"] {
            assert!(parse(Parser::AccountingStats, "cpuacct.stat", raw).is_err());
        }
    }
}
