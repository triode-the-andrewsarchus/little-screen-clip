//! Semantics come from device identity, never from consuming a device stream.
use super::{Document, Relation, model::Fact};
use little_clip_core::kernel_interface::Kind;
use std::path::PathBuf;

const RANDOM: &str =
    "Kernel cryptographically secure random-number interface. Preview consumes no random data.";
const DESCRIPTIONS: &[((u32, u32), &str)] = &[
    (
        (1, 3),
        "Discard sink / EOF source. Writes discard data; reads return EOF.",
    ),
    (
        (1, 5),
        "Infinite zero-byte source. Reads produce zero bytes; writes discard data.",
    ),
    (
        (1, 7),
        "Simulated full device. Writes fail with ENOSPC; reads produce zero bytes.",
    ),
    ((1, 8), RANDOM),
    ((1, 9), RANDOM),
    (
        (1, 11),
        "Kernel message interface. Preview does not open or consume kernel records.",
    ),
    (
        (1, 1),
        "Physical memory access. No automatic reads or writes.",
    ),
    ((1, 4), "Raw I/O port access. No automatic reads or writes."),
    (
        (5, 0),
        "Calling process's controlling terminal. No terminal input is read.",
    ),
    (
        (5, 1),
        "System console interface. No console input is read.",
    ),
    (
        (5, 2),
        "PTY allocator. Opening allocates a master/slave pair; preview never opens it.",
    ),
    (
        (10, 229),
        "Userspace filesystem interface (FUSE). Preview never opens it.",
    ),
];

pub(super) fn describe(document: &mut Document) {
    let Kind::Character { major, minor } = document.kind else {
        return;
    };
    if (major, minor) == (1, 11) {
        document.title = "Kernel message interface · 1:11".into();
    }
    if let Some((_, description)) = DESCRIPTIONS
        .iter()
        .find(|(number, _)| *number == (major, minor))
    {
        let mut fact = Fact::text("Interface semantics", *description, document.path.clone());
        fact.interface = true;
        document.facts.insert(0, fact);
    }
}

/// Collects conventional log locations only inside the bounded semantic helper.
pub(super) fn collect(document: &mut Document) {
    if document.kind
        == (Kind::Character {
            major: 1,
            minor: 11,
        })
    {
        log_locations(
            document,
            [
                "/var/log/kern.log",
                "/var/log/messages",
                "/var/log/syslog",
                "/var/log/journal",
                "/run/log/journal",
            ]
            .into_iter()
            .map(PathBuf::from),
        );
    }
}

fn log_locations(document: &mut Document, candidates: impl IntoIterator<Item = PathBuf>) {
    let mut found = 0;
    for path in candidates.into_iter().take(5) {
        if document
            .relations
            .iter()
            .any(|relation| relation.target == path)
        {
            found += 1;
            continue;
        }
        if let Err(error) = little_clip_core::kernel_interface::pin(&path, true) {
            if error.kind() != std::io::ErrorKind::NotFound {
                document
                    .facts
                    .push(Fact::failure("Kernel log location", path, error));
            }
            continue;
        }
        let mut relation = Relation::new("Kernel log location", path);

        if relation.identity.is_none()
            || !relation
                .context
                .is_some_and(|context| context.kind == Kind::Content)
        {
            document.facts.push(Fact::failure(
                "Kernel log location",
                relation.target,
                std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "Ordinary log target identity is unavailable or protected",
                ),
            ));
            continue;
        }
        if relation.directory {
            relation.label = "System journal directory".into();
        }
        document.relations.push(relation);
        found += 1;
    }
    let fact = Fact::text(
        "Kernel log inspection",
        format!(
            "{found} conventional local log location(s) exposed; links below browse/reveal files. Locations may include non-kernel messages and are not a complete log history. Readability is unverified; normal account permissions apply. No log content was read."
        ),
        document.path.clone(),
    );
    document.facts.push(fact);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn pseudo_registry_uses_device_identity_instead_of_filename_or_payload() {
        let original =
            super::super::worker::interface(std::path::Path::new("/proc/uptime")).unwrap();
        for &((major, minor), description) in DESCRIPTIONS {
            let mut document = original.clone();
            document.kind = Kind::Character { major, minor };
            document.path = "/misleading/ordinary-name".into();
            describe(&mut document);
            let fact = document
                .facts
                .iter()
                .find(|fact| fact.label == "Interface semantics")
                .unwrap();
            assert_eq!(fact.value.as_deref(), Some(description));
            assert_eq!(fact.source, document.path);
            assert!(fact.interface);
        }
        for kind in [
            Kind::Content,
            Kind::Block { major: 1, minor: 3 },
            Kind::Character {
                major: 99,
                minor: 3,
            },
        ] {
            let mut document = original.clone();
            document.kind = kind;
            document.path = "/dev/null".into();
            describe(&mut document);
            assert!(
                !document
                    .facts
                    .iter()
                    .any(|fact| fact.label == "Interface semantics")
            );
        }
    }

    #[test]
    fn denied_log_metadata_is_not_reported_as_an_empty_successful_inventory() {
        use std::os::unix::fs::PermissionsExt;
        // A root-run suite cannot manufacture an ordinary DAC denial this way.
        // SAFETY: geteuid only queries this process's credentials.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let locked = root.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        let log = locked.join("kern.log");
        std::fs::write(&log, b"not read").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o0)).unwrap();
        let mut document =
            super::super::worker::interface(std::path::Path::new("/proc/uptime")).unwrap();
        document.relations.clear();
        log_locations(&mut document, [log.clone()]);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(document.relations.is_empty());
        assert!(document.facts.iter().any(|fact| fact.source == log
            && fact.outcome == super::super::model::Outcome::PermissionDenied));
    }

    #[test]
    fn kernel_log_locations_keep_ordinary_identity_and_reject_kernel_aliases() {
        let root = tempfile::tempdir().unwrap();
        let log = root.path().join("kern.log");
        let journal = root.path().join("journal");
        let forbidden = root.path().join("kernel-alias");
        std::fs::write(&log, b"Content must not enter metadata facts").unwrap();
        std::fs::create_dir(&journal).unwrap();
        symlink("/proc/uptime", &forbidden).unwrap();
        let mut document =
            super::super::worker::interface(std::path::Path::new("/proc/uptime")).unwrap();
        document.relations.clear();
        log_locations(
            &mut document,
            [
                log.clone(),
                journal.clone(),
                forbidden,
                root.path().join("missing"),
            ],
        );
        assert_eq!(document.relations.len(), 2);
        assert!(!document.relations[0].system);
        assert!(!document.relations[0].directory);
        assert!(document.relations[1].directory);
        assert!(document.relations.iter().all(|relation| {
            relation
                .context
                .is_some_and(|context| context.kind == Kind::Content)
        }));
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label == "Kernel log inspection" && !fact.interface),
            "helper observations must survive cache composition"
        );
        let captured = document.relations[0].clone();
        assert!(captured.verify().is_ok());
        std::fs::rename(&log, root.path().join("previous")).unwrap();
        std::fs::write(&log, b"replacement").unwrap();
        assert!(captured.verify().is_err());
        log_locations(&mut document, [log, journal]);
        assert_eq!(
            document.relations.len(),
            2,
            "repeated inventories must not duplicate log links"
        );
        assert!(!document.facts.iter().any(|fact| {
            fact.value
                .as_deref()
                .is_some_and(|value| value.contains("Content must not"))
        }));
    }
}
