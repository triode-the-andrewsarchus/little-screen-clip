//! Visible ancestor configuration is context, not calculated effective capacity.
use super::{Document, Fact, Outcome, Reads, Version, observe};
use crate::file_manager::system::identity::MountIdentity;
use std::{io, path::Path};

const MAX_ANCESTORS: usize = 6;
const LIMITS: &[&str] = &[
    "cpu.max",
    "memory.max",
    "memory.high",
    "memory.swap.max",
    "pids.max",
    "cgroup.subtree_control",
    "cpu.cfs_quota_us",
    "cpu.cfs_period_us",
    "memory.limit_in_bytes",
    "memory.memsw.limit_in_bytes",
];

pub(super) fn collect(
    document: &mut Document,
    root: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
    version: Version,
) {
    collect_with(
        document,
        root,
        reads,
        checkpoint,
        version,
        MountIdentity::directory,
    );
}

fn collect_with(
    document: &mut Document,
    root: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
    version: Version,
    directory: impl Fn(&Path) -> io::Result<MountIdentity>,
) {
    let result = scan(document, root, reads, checkpoint, version, &directory);
    let fact = match result {
        Ok((count, truncated)) => {
            let mut fact = Fact::text(
                "Ancestor constraint coverage",
                format!(
                    "{count} visible parents inspected{}; stops at mount boundaries. Hidden ancestors may still constrain this group. Per-field errors remain unavailable. These are shared ancestor settings, not effective limits or available headroom; observations are not atomic.",
                    if truncated {
                        " (six-parent limit reached)"
                    } else {
                        ""
                    }
                ),
                root.into(),
            );
            if truncated {
                fact.outcome = Outcome::Truncated;
            }
            fact
        }
        Err(error) => Fact::failure("Ancestor constraint coverage", root.into(), error),
    };
    document.facts.push(fact);
    if matches!(version, Version::Unified) {
        delegation_mount(document, root, reads);
        document.facts.push(Fact::text(
            "Controller and delegation context",
            "Available controllers and controllers enabled for children describe configuration, not permission to delegate. Ownership and mode bits do not establish write access, task-migration rights, ACL/LSM approval or namespace delegation. No delegation or migration is attempted.",
            root.into(),
        ));
    }
    checkpoint(document);
}

/// Report mount configuration without turning nsdelegate or permissions into an access badge.
fn delegation_mount(document: &mut Document, root: &Path, reads: &mut Reads) {
    let result = (|| {
        let identity = MountIdentity::directory(root)?;
        let inventory = crate::file_manager::system::storage::mounts::parse(&reads.mountinfo()?);
        let mount = inventory
            .records
            .iter()
            .find(|mount| {
                mount.id == identity.mount
                    && mount.filesystem == "cgroup2"
                    && mount.device == (libc::major(identity.device), libc::minor(identity.device))
            })
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "Matching cgroup v2 mount is not visible",
                )
            })?;
        if MountIdentity::directory(root)? != identity {
            return Err(io::Error::other(
                "Cgroup mount changed while reading delegation context",
            ));
        }
        let mut fact = Fact::text(
            "Cgroup v2 mount options",
            mount
                .super_options
                .iter()
                .take(32)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            "/proc/self/mountinfo".into(),
        );
        fact.outcome = if mount.super_options.len() > 32 {
            Outcome::Truncated
        } else {
            inventory.outcome
        };
        let mut delegation = Fact::text(
            "Namespace delegation setting",
            if mount
                .super_options
                .iter()
                .any(|option| option == "nsdelegate")
            {
                "nsdelegate is reported for this mount; this describes namespace delegation configuration, not this user's access or proof that this group is delegated"
            } else {
                "nsdelegate is not reported in this mount record; delegation through granted directory/interface access remains possible"
            },
            "/proc/self/mountinfo".into(),
        );
        delegation.outcome = fact.outcome.clone();
        Ok([fact, delegation])
    })();
    match result {
        Ok(facts) => document.facts.extend(facts),
        Err(error) => document.facts.push(Fact::failure(
            "Namespace delegation setting",
            "/proc/self/mountinfo".into(),
            error,
        )),
    }
}

fn scan(
    document: &mut Document,
    root: &Path,
    reads: &mut Reads,
    checkpoint: &mut dyn FnMut(&Document),
    version: Version,
    directory: &impl Fn(&Path) -> io::Result<MountIdentity>,
) -> io::Result<(usize, bool)> {
    let scope = directory(root)?;
    let mut count = 0;
    for parent in root.ancestors().skip(1) {
        let identity = directory(parent)?;
        if !scope.same_hierarchy_mount(identity) {
            break;
        }
        if count == MAX_ANCESTORS {
            return Ok((count, true));
        }
        let facts = version
            .fields()
            .iter()
            .filter(|(name, _, _)| LIMITS.contains(name))
            .map(|field| {
                let mut fact = observe(reads, parent, field, version);
                fact.label = format!("Ancestor {} · {}", count + 1, fact.label);
                fact
            })
            .collect::<Vec<_>>();
        if directory(root)? != scope || directory(parent)? != identity {
            return Err(io::Error::other(
                "Cgroup ancestor mount/object changed during observation",
            ));
        }
        document.facts.extend(facts);
        count += 1;
        checkpoint(document);
    }
    Ok((count, false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn fixture(
        root: &Path,
        values: BTreeMap<std::path::PathBuf, Vec<u8>>,
        version: Version,
        directory: impl Fn(&Path) -> io::Result<MountIdentity>,
    ) -> (Document, Reads) {
        let mut document =
            crate::file_manager::system::worker::interface(Path::new("/proc/uptime")).unwrap();
        document.path = root.into();
        document.interface_directory = true;
        let mut reads = Reads::fixture(values);
        collect_with(
            &mut document,
            root,
            &mut reads,
            &mut |_| {},
            version,
            directory,
        );
        (document, reads)
    }

    fn scope(path: &Path) -> io::Result<MountIdentity> {
        Ok(MountIdentity {
            device: 1,
            inode: path.as_os_str().len() as u64,
            mount: 2,
        })
    }

    #[test]
    fn visible_ancestors_preserve_limits_sources_failures_and_shared_scope() {
        let root = Path::new("/groups/parent/leaf");
        let values = BTreeMap::from([
            (
                Path::new("/groups/parent/cpu.max").into(),
                b"200000 100000".to_vec(),
            ),
            (
                Path::new("/groups/parent/memory.max").into(),
                b"max".to_vec(),
            ),
            (Path::new("/groups/cpu.max").into(), b"bad".to_vec()),
            (Path::new("/groups/pids.max").into(), b"100".to_vec()),
        ]);
        let (document, reads) = fixture(root, values, Version::Unified, |path| {
            let mut id = scope(path)?;
            if path == Path::new("/") {
                id.mount = 3;
            }
            Ok(id)
        });
        let cpu = document
            .facts
            .iter()
            .find(|f| f.label == "Ancestor 1 · Local CPU bandwidth limit")
            .unwrap();
        assert_eq!(cpu.source, Path::new("/groups/parent/cpu.max"));
        assert!(cpu.value.as_ref().unwrap().contains("2.00 CPU equivalents"));
        assert!(
            document
                .facts
                .iter()
                .any(|f| f.label == "Ancestor 2 · Local CPU bandwidth limit"
                    && f.outcome == Outcome::Malformed)
        );
        assert!(
            document.facts.iter().any(
                |f| f.label == "Ancestor 1 · Local swap limit" && f.outcome == Outcome::Missing
            )
        );
        assert!(
            document
                .facts
                .iter()
                .any(|f| f.label == "Ancestor constraint coverage"
                    && f.value.as_ref().unwrap().contains("2 visible parents"))
        );
        assert!(
            reads
                .attempts()
                .all(|p| !p.starts_with("/cpu.max") && super::super::approved(p))
        );
    }

    #[test]
    fn deep_hierarchies_are_bounded_and_do_not_read_members_or_usage() {
        let root = Path::new("/a/b/c/d/e/f/g/h/i");
        let (document, reads) = fixture(root, BTreeMap::new(), Version::Legacy, scope);
        assert!(
            document
                .facts
                .iter()
                .any(|f| f.label == "Ancestor constraint coverage"
                    && f.outcome == Outcome::Truncated)
        );
        assert!(
            reads
                .attempts()
                .all(|p| LIMITS.contains(&p.file_name().unwrap().to_str().unwrap()))
        );
        assert!(reads.attempts().count() <= 30);
        assert!(
            !document
                .facts
                .iter()
                .any(|f| f.label.starts_with("Ancestor 7"))
        );
    }

    #[test]
    fn delegation_options_match_mount_identity_and_preserve_permission_uncertainty() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let identity = MountIdentity::directory(root).unwrap();
        let record = format!(
            "{} 1 {}:{} / /different-visible-path rw - cgroup2 cgroup rw,nsdelegate,memory_recursiveprot\n",
            identity.mount,
            libc::major(identity.device),
            libc::minor(identity.device)
        );
        let (document, _) = fixture(
            root,
            BTreeMap::from([(
                Path::new("/proc/self/mountinfo").into(),
                record.into_bytes(),
            )]),
            Version::Unified,
            MountIdentity::directory,
        );
        assert!(
            document
                .facts
                .iter()
                .any(|f| f.label == "Namespace delegation setting"
                    && f.outcome == Outcome::Observed
                    && f.value.as_ref().unwrap().contains("not this user's access"))
        );
        assert!(
            document
                .facts
                .iter()
                .any(|f| f.label == "Cgroup v2 mount options"
                    && f.value.as_ref().unwrap().contains("memory_recursiveprot"))
        );
    }

    #[test]
    fn denied_ancestor_metadata_does_not_admit_payload_reads() {
        let root = Path::new("/groups/leaf");
        let (document, reads) = fixture(root, BTreeMap::new(), Version::Unified, |path| {
            if path == root {
                scope(path)
            } else {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            }
        });
        assert_eq!(reads.attempts().count(), 0);
        assert!(
            document
                .facts
                .iter()
                .any(|f| f.label == "Ancestor constraint coverage"
                    && f.outcome == Outcome::PermissionDenied)
        );
    }

    #[test]
    fn changing_ancestor_identity_rejects_its_facts() {
        let root = Path::new("/a/b");
        let calls = std::cell::Cell::new(0);
        let (document, _) = fixture(root, BTreeMap::new(), Version::Unified, |p| {
            let mut id = scope(p)?;
            if p == Path::new("/a") {
                calls.set(calls.get() + 1);
                id.inode += calls.get();
            }
            Ok(id)
        });
        assert!(
            !document
                .facts
                .iter()
                .any(|f| f.label.starts_with("Ancestor 1"))
        );
        assert!(document.facts.iter().any(
            |f| f.label == "Ancestor constraint coverage" && f.outcome == Outcome::Unsupported
        ));
    }
}
