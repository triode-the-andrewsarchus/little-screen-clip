//! Passive tracing configuration and limited security state, never trace/policy data.
use super::{
    Document,
    attributes::{self, Format, Rule},
    buses::{self, Topology},
    collector::Reads,
    model::{Fact, Outcome},
};
use little_clip_core::kernel_interface::{Filesystem, Kind};
use std::path::Path;

const TRACE: &[(&str, &str, Format)] = &[
    ("current_tracer", "Configured tracer", Format::Text),
    ("available_tracers", "Available tracers", Format::Text),
    (
        "tracing_on",
        "Ring-buffer recording",
        Format::Boolean("Enabled", "Disabled"),
    ),
    ("trace_clock", "Selected trace clock", Format::Selection),
    (
        "buffer_size_kb",
        "Per-CPU buffer size (kernel kB)",
        Format::Text,
    ),
    (
        "buffer_total_size_kb",
        "Total buffer size (kernel kB)",
        Format::Text,
    ),
];
const SECURITY: &[(&str, &str, Format)] = &[
    ("lsm", "Kernel-reported security modules", Format::Text),
    ("lockdown", "Kernel lockdown selection", Format::Selection),
];

const FUSE: &[(&str, &str, Format)] = &[
    (
        "waiting",
        "Outstanding userspace requests",
        Format::Unsigned,
    ),
    (
        "max_background",
        "Background request limit",
        Format::Unsigned,
    ),
    (
        "congestion_threshold",
        "Congestion threshold",
        Format::Unsigned,
    ),
];
fn fuse_connection(root: &Path, reference: &Path) -> Option<u64> {
    let id = root
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.parse::<u64>().ok());
    if id.is_some()
        && root.parent().is_some_and(|parent| {
            little_clip_core::kernel_interface::classify(parent).ok()
                == Some(Kind::Kernel {
                    filesystem: Filesystem::FuseControl,
                })
        })
    {
        return id;
    }
    // A bind-mounted connection can have an arbitrary name. Resolve by metadata,
    // never by consuming a connection file or probing its filesystem daemon.
    std::fs::read_dir(reference)
        .ok()?
        .take(128)
        .filter_map(Result::ok)
        .find_map(|entry| {
            let id = entry.file_name().to_str()?.parse::<u64>().ok()?;
            super::boot_firmware::same_object(root, &entry.path()).then_some(id)
        })
}

pub(super) fn approved(path: &Path, filesystem: Filesystem) -> bool {
    let fields = match filesystem {
        Filesystem::Trace => TRACE,
        Filesystem::Security => SECURITY,
        Filesystem::FuseControl => FUSE,
        _ => return false,
    };
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| fields.iter().any(|(field, _, _)| *field == name))
}
fn trace_configuration(root: &Path) -> bool {
    ["/sys/kernel/tracing", "/sys/kernel/debug/tracing"]
        .iter()
        .any(|reference| super::boot_firmware::same_object(root, Path::new(reference)))
        || ["current_tracer", "tracing_on"].iter().all(|name| {
            std::fs::symlink_metadata(root.join(name)).is_ok_and(|metadata| metadata.is_file())
        })
}
pub(super) fn collect(
    document: &mut Document,
    root: &Path,
    checkpoint: &mut dyn FnMut(&Document),
) -> bool {
    collect_at(
        document,
        root,
        Path::new("/sys/kernel/security"),
        Path::new("/sys/fs/fuse/connections"),
        &mut Reads::new(),
        checkpoint,
    )
}
fn collect_at(
    document: &mut Document,
    root: &Path,
    security_root: &Path,
    fuse_root: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
) -> bool {
    let Kind::Kernel { filesystem } = document.kind else {
        return false;
    };
    let (title, description, fields) = match filesystem {
        Filesystem::Trace => (
            "Tracing facilities",
            "Tracefs exposes tracer configuration and event facilities. Recording disabled does not prove that all tracing overhead has stopped. Trace, trace_pipe, snapshots and raw buffers are never opened for payloads.",
            if trace_configuration(root) {
                TRACE
            } else {
                &[][..]
            },
        ),
        Filesystem::Security => (
            "Security subsystem state",
            "Only limited kernel-reported state is inspected. Readable attributes do not establish the complete security posture. Policies, keys, labels, measurements and logs remain metadata-only.",
            if super::boot_firmware::same_object(root, security_root) {
                SECURITY
            } else {
                &[][..]
            },
        ),
        Filesystem::FuseControl => (
            "FUSE control connections",
            "Connection identifiers and request counts are observations, not permanent identities. Outstanding requests alone do not prove a hung daemon. Background and congestion limits are configuration, not current load. Abort is never opened for payloads and no connection setting is changed.",
            if let Some(id) = fuse_connection(root, fuse_root) {
                document.facts.push(Fact::text(
                    "Connection identifier",
                    id.to_string(),
                    root.into(),
                ));
                FUSE
            } else {
                &[][..]
            },
        ),
        Filesystem::Debug => (
            "Debug interfaces",
            "Debugfs callbacks are driver-specific and may have side effects even when read. This preview inventories names and metadata only; no debug payload or control is opened.",
            &[][..],
        ),
        Filesystem::Config => (
            "Kernel configuration objects",
            "Configfs directories and attributes configure kernel objects. Creation, removal and writes can change live kernel state. This preview reads no attribute payloads and creates no objects.",
            &[][..],
        ),
        Filesystem::Bpf => (
            "Pinned BPF objects",
            "Bpffs names reference pinned kernel BPF objects, not ordinary file contents. This preview does not acquire object handles, query maps/programs, or unpin objects.",
            &[][..],
        ),
        Filesystem::Pstore => (
            "Persistent kernel records",
            "Pstore exposes persistent crash and firmware records. Contents may be sensitive; removing records can erase persistent evidence. This preview reads no record payloads and removes nothing.",
            &[][..],
        ),
        Filesystem::ResourceControl => (
            "Resource-control interfaces",
            "Resctrl exposes resource-allocation and monitoring groups. This preview does not read monitoring counters, assign tasks, change allocations, or create/remove groups.",
            &[][..],
        ),
        Filesystem::Binder => (
            "Binder device interfaces",
            "Binderfs exposes binder devices and control interfaces. This preview opens no binder endpoint for I/O and creates/removes no devices.",
            &[][..],
        ),
        _ => return false,
    };
    document.title = title.into();
    document.facts.push(Fact::text(
        "Passive inspection policy",
        description,
        root.into(),
    ));
    checkpoint(document);
    let requested = document.requested_attribute();
    for (name, label, format) in fields.iter().filter(|(name, _, _)| {
        requested
            .as_deref()
            .is_none_or(|requested| requested == *name)
    }) {
        let rule = Rule::new(name, label, *format);
        document.facts.push(observe(reads, root, filesystem, &rule));
        checkpoint(document);
    }
    if requested
        .as_deref()
        .is_some_and(|requested| !fields.iter().any(|(name, _, _)| requested == *name))
    {
        document.facts.push(Fact::text(
            "Content policy",
            "Unsupported kernel interface; payload not read",
            document.path.clone(),
        ));
    }
    if document.interface_directory {
        inventory(document, root, title);
    }
    document
        .facts
        .sort_by_key(|fact| (fact.interface, fact.label == "Passive inspection policy"));
    checkpoint(document);
    true
}
fn observe(reads: &mut Reads, root: &Path, filesystem: Filesystem, rule: &Rule) -> Fact {
    let source = root.join(&rule.name);
    Fact::attribute(
        &rule.label,
        source.clone(),
        format!("documented {filesystem:?} attribute"),
        reads.kernel_state(&source, filesystem),
        |raw| attributes::parse(rule, raw),
    )
}

fn inventory(document: &mut Document, root: &Path, title: &str) {
    let mut topology = Topology::new(title, buses::attribute_summary(document, root));
    buses::add(&mut topology, document, root.into(), title.into(), None);
    match std::fs::read_dir(root) {
        Ok(entries) => {
            let (children, coverage) = buses::select_children(
                entries,
                |entry| entry.file_type().is_ok_and(|kind| kind.is_dir()),
                16,
            );
            topology.coverage = coverage;
            for child in children {
                buses::add_descendant(&mut topology, document, child, 0);
            }
        }
        Err(error) => {
            topology.coverage = Outcome::Unsupported;
            document.facts.push(Fact::failure(
                "Interface directory inventory",
                root.into(),
                error,
            ));
        }
    }
    document.facts.push(Fact::text("Inventory coverage", format!("{} immediate directories retained; at most 128 entries scanned and 16 retained; no recursive traversal", topology.nodes.len().saturating_sub(1)), root.into()));
    document.topology = Some(topology);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io, path::PathBuf};
    fn fixture(
        filesystem: Filesystem,
        values: &[(&str, &str)],
    ) -> (tempfile::TempDir, PathBuf, Reads, Document) {
        let (temp, root, reads, mut document) =
            super::super::collector::registered_fixture("fixture", "interfaces", values);
        document.kind = Kind::Kernel { filesystem };
        document.path = root.clone();
        document.interface_directory = true;
        (temp, root, reads, document)
    }
    fn assert_metadata_only(
        document: &mut Document,
        root: &Path,
        security: &Path,
        reads: &mut Reads,
    ) {
        let attempts = reads.attempts().count();
        collect_at(
            document,
            root,
            security,
            Path::new("/sys/fs/fuse/connections"),
            reads,
            &mut |_| {},
        );
        assert_eq!(reads.attempts().count(), attempts);
    }

    #[test]
    fn fuse_limits_and_waiting_are_snapshots_and_abort_is_never_read() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("91");
        std::fs::create_dir(&root).unwrap();
        let values = [
            ("waiting", "3"),
            ("max_background", "12"),
            ("congestion_threshold", "9"),
            ("abort", "secret"),
        ];
        let mut reads = Reads::fixture(
            values
                .into_iter()
                .map(|(name, value)| (root.join(name), value.as_bytes().to_vec()))
                .collect(),
        );
        let mut document = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        document.kind = Kind::Kernel {
            filesystem: Filesystem::FuseControl,
        };
        document.path = root.clone();
        document.interface_directory = true;
        assert!(collect_at(
            &mut document,
            &root,
            temp.path(),
            temp.path(),
            &mut reads,
            &mut |_| {}
        ));
        assert_eq!(document.observed_raw("Connection identifier"), Some("91"));
        assert_eq!(
            document.observed_raw("Outstanding userspace requests"),
            Some("3")
        );
        assert_eq!(
            document.observed_raw("Background request limit"),
            Some("12")
        );
        assert_eq!(reads.attempts().count(), 3);
        assert!(!reads.attempts().any(|path| path.ends_with("abort")));
        document.path = root.join("abort");
        document.interface_directory = false;
        collect_at(
            &mut document,
            &root,
            temp.path(),
            temp.path(),
            &mut reads,
            &mut |_| {},
        );
        assert_eq!(reads.attempts().count(), 3);
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "Content policy")
        );
    }

    #[test]
    fn fuse_connection_aliases_resolve_by_object_and_unverified_names_do_not_authorize_callbacks() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("17");
        std::fs::create_dir(&root).unwrap();
        let alias = temp.path().join("connection-alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        assert_eq!(fuse_connection(&alias, temp.path()), Some(17));
        let unrelated = temp.path().join("unrelated");
        std::fs::create_dir(&unrelated).unwrap();
        assert_eq!(fuse_connection(&unrelated, temp.path()), None);
        assert!(!approved(&root.join("abort"), Filesystem::FuseControl));
        assert!(!approved(&root.join("unknown"), Filesystem::FuseControl));
    }

    #[test]
    fn tracing_configuration_never_opens_streams_or_snapshots() {
        let (_temp, root, mut reads, mut document) = fixture(
            Filesystem::Trace,
            &[
                ("current_tracer", "nop"),
                ("tracing_on", "0"),
                ("trace_clock", "local [global] counter"),
                ("buffer_size_kb", "0 (expanded: 1408)"),
                ("trace_pipe", "secret"),
                ("trace", "secret"),
                ("snapshot", "secret"),
            ],
        );
        assert!(collect_at(
            &mut document,
            &root,
            Path::new("/unused"),
            Path::new("/sys/fs/fuse/connections"),
            &mut reads,
            &mut |_| {}
        ));
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "Ring-buffer recording"
                    && fact.value.as_deref() == Some("Disabled"))
        );
        assert!(
            reads
                .attempts()
                .all(|path| approved(path, Filesystem::Trace))
        );
        for name in [
            "trace",
            "trace_pipe",
            "trace_pipe_raw",
            "snapshot",
            "set_ftrace_filter",
            "kprobe_events",
        ] {
            assert!(!approved(&root.join(name), Filesystem::Trace));
        }
    }
    #[test]
    fn trace_stream_selection_and_unknown_nested_configuration_stay_payload_free() {
        let (_temp, root, mut reads, mut document) =
            fixture(Filesystem::Trace, &[("trace_pipe", "secret")]);
        document.interface_directory = false;
        document.path = root.join("trace_pipe");
        assert_metadata_only(&mut document, &root, Path::new("/unused"), &mut reads);
        assert!(
            document.facts.iter().any(|fact| fact.value.as_deref()
                == Some("Unsupported kernel interface; payload not read"))
        );
    }
    #[test]
    fn security_state_requires_the_verified_root_and_does_not_describe_overall_posture() {
        let (temp, root, mut reads, mut document) = fixture(
            Filesystem::Security,
            &[
                ("lsm", "capability,landlock,lockdown"),
                ("lockdown", "[none] integrity confidentiality"),
                ("policy", "secret"),
            ],
        );
        collect_at(
            &mut document,
            &root,
            &root,
            Path::new("/sys/fs/fuse/connections"),
            &mut reads,
            &mut |_| {},
        );
        assert_eq!(reads.attempts().count(), 2);
        assert_eq!(document.facts[0].label, "Kernel-reported security modules");
        assert!(
            document
                .topology
                .as_ref()
                .unwrap()
                .summary
                .contains("Kernel-reported security modules")
        );
        assert!(
            !document
                .topology
                .as_ref()
                .unwrap()
                .summary
                .contains("complete security posture")
        );
        assert!(document.facts.iter().any(|fact| {
            fact.label == "Kernel lockdown selection"
                && fact
                    .value
                    .as_deref()
                    .is_some_and(|value| value.starts_with("none ·"))
        }));
        let nested = root.join("vendor");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("lsm"), "secret").unwrap();
        let mut other = Reads::fixture(std::collections::BTreeMap::from([(
            nested.join("lsm"),
            b"secret".to_vec(),
        )]));
        document.path = nested.clone();
        document.facts.clear();
        collect_at(
            &mut document,
            &nested,
            &root,
            Path::new("/sys/fs/fuse/connections"),
            &mut other,
            &mut |_| {},
        );
        assert_eq!(other.attempts().count(), 0);
        drop(temp);
    }
    #[test]
    fn debug_payloads_are_not_read_and_directory_inventory_is_bounded() {
        let (_temp, root, mut reads, mut document) = fixture(
            Filesystem::Debug,
            &[("current_tracer", "secret"), ("tracing_on", "1")],
        );
        for index in 0..20 {
            std::fs::create_dir(root.join(format!("interface{index}"))).unwrap();
        }
        assert_metadata_only(&mut document, &root, Path::new("/unused"), &mut reads);
        assert_eq!(
            document.topology.as_ref().unwrap().coverage,
            Outcome::Truncated
        );
        assert_eq!(document.topology.as_ref().unwrap().nodes.len(), 17);
    }
    #[test]
    fn object_control_and_persistent_record_previews_are_payload_free() {
        for filesystem in [
            Filesystem::Config,
            Filesystem::Bpf,
            Filesystem::Pstore,
            Filesystem::ResourceControl,
            Filesystem::Binder,
        ] {
            let (_temp, root, mut reads, mut document) = fixture(
                filesystem,
                &[
                    ("lsm", "secret"),
                    ("current_tracer", "secret"),
                    ("tracing_on", "1"),
                ],
            );
            assert!(collect_at(
                &mut document,
                &root,
                &root,
                Path::new("/sys/fs/fuse/connections"),
                &mut reads,
                &mut |_| {}
            ));
            assert_eq!(reads.attempts().count(), 0);
            document.interface_directory = false;
            document.path = root.join("lsm");
            document.facts.clear();
            assert_metadata_only(&mut document, &root, &root, &mut reads);
            assert!(document.facts.iter().any(|fact| fact.value.as_deref()
                == Some("Unsupported kernel interface; payload not read")));
        }
    }
    #[test]
    fn configuration_reader_rejects_ordinary_files_even_with_matching_names() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("current_tracer");
        std::fs::write(&path, "nop").unwrap();
        assert_eq!(
            Reads::new()
                .kernel_state(&path, Filesystem::Trace)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Unsupported
        );
    }
}
