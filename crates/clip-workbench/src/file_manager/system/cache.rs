//! Alias-coalesced snapshot ownership; a request never duplicates an in-flight read.
use super::{
    Document,
    identity::{Key, observation_identity},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{
        Arc, Condvar, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RefreshPolicy {
    Row,
    Inspector,
    Manual,
}
impl RefreshPolicy {
    fn freshness(self) -> Duration {
        match self {
            Self::Row => Duration::from_secs(5),
            Self::Inspector => Duration::from_secs(1),
            Self::Manual => Duration::ZERO,
        }
    }
}
struct Snapshot {
    document: Arc<Document>,
    received: Instant,
    policy: RefreshPolicy,
}
#[derive(Default)]
struct State {
    ready: BTreeMap<Key, Snapshot>,
    loading: BTreeSet<Key>,
}
static STATE: LazyLock<(Mutex<State>, Condvar)> =
    LazyLock::new(|| (Mutex::new(State::default()), Condvar::new()));
pub(in crate::file_manager) fn collect(
    path: &Path,
    token: &AtomicBool,
) -> Result<Document, String> {
    collect_for(path, token, RefreshPolicy::Manual)
}
pub(super) fn collect_for(
    path: &Path,
    token: &AtomicBool,
    policy: RefreshPolicy,
) -> Result<Document, String> {
    observe(path, token, policy).map(|document| (*document).clone())
}
pub(super) fn preview(path: &Path, token: &AtomicBool) -> Result<Arc<Document>, String> {
    observe(path, token, RefreshPolicy::Row)
}
fn request_key(path: &Path) -> Result<(Key, Document), String> {
    let interface = super::worker::interface(path)?;
    let key = Key::for_document(&interface);
    Ok((key, interface))
}
fn compose(mut interface: Document, observation: &Document) -> Arc<Document> {
    if interface.path == observation.path {
        interface.title = observation.title.clone();
    }
    interface.facts.extend(
        observation
            .facts
            .iter()
            .filter(|fact| !fact.interface)
            .cloned(),
    );
    interface.facts.sort_by_key(|fact| fact.interface);
    for relation in &observation.relations {
        if !interface
            .relations
            .iter()
            .any(|current| current.target == relation.target)
        {
            interface.relations.push(relation.clone());
        }
    }
    super::console::describe(&mut interface);
    super::pseudo::describe(&mut interface);
    interface.observed_ms = observation.observed_ms;
    interface.event_revision = observation.event_revision;
    interface.generation = observation.generation;
    interface.last_error = observation.last_error.clone();
    interface.storage = observation.storage.clone();
    interface.topology = observation.topology.clone();
    interface.telemetry = observation
        .telemetry
        .as_ref()
        .and_then(|data| super::telemetry::assemble(&interface, data.kind, &data.root));
    if interface.interface_directory {
        if let Some(data) = &interface.telemetry {
            interface.title = data.role.clone();
        }
    }
    interface.cpu = observation.cpu.clone();
    interface.numa = observation.numa.clone();
    if interface.interface_directory
        && matches!(
            interface.kind,
            little_clip_core::kernel_interface::Kind::Kernel {
                filesystem: little_clip_core::kernel_interface::Filesystem::Sysfs
            }
        )
    {
        if let Some(cpu) = &interface.cpu {
            interface.title = cpu.role.clone();
        }
    }
    if let Some(storage) = &interface.storage {
        interface.title = format!("{} · {}", storage.class, storage.name);
    }
    super::processes::attach(&mut interface);
    interface.stamp_fact_generations();
    Arc::new(interface)
}
fn reusable_static(snapshot: &Snapshot, fingerprint: &[u64]) -> bool {
    snapshot.document.namespace.known()
        && snapshot.document.observation_context().is_some()
        && observation_identity(&snapshot.document) == fingerprint
        && kernel_current(&snapshot.document)
        && !snapshot.document.invalidated()
}
fn valid(
    snapshot: &Snapshot,
    fingerprint: &[u64],
    entered: Instant,
    policy: RefreshPolicy,
) -> bool {
    reusable_static(snapshot, fingerprint)
        && (policy != RefreshPolicy::Manual || snapshot.policy == RefreshPolicy::Manual)
        && (snapshot.received >= entered || snapshot.received.elapsed() < policy.freshness())
}
fn kernel_current(document: &Document) -> bool {
    let Some(expected) = &document.kernel_identity else {
        return true;
    };
    let Some(path) = document
        .relations
        .iter()
        .find(|r| r.label == "Kernel object")
        .map(|r| &r.target)
    else {
        return false;
    };
    let Ok(file) = little_clip_core::kernel_interface::pin(path, true) else {
        return false;
    };
    file.metadata()
        .is_ok_and(|stat| super::identity::fingerprint(&stat) == *expected)
        && super::identity::ObjectContext::pinned(&file).ok() == document.kernel_context
        && document.kernel_context.is_some()
}

/// Preserve a failed attempt only across a revalidated, unchanged lifetime.
fn retained_failure(
    mut current: Document,
    previous: &Document,
    error: &str,
) -> Option<Arc<Document>> {
    if !current.namespace.known()
        || Key::for_document(&current) != Key::for_document(previous)
        || observation_identity(&current) != observation_identity(previous)
        || !kernel_current(previous)
        || previous.invalidated()
    {
        return None;
    }
    current.event_revision = super::events::revision();
    super::identity::stamp(&mut current);
    if current.generation != previous.generation {
        return None;
    }
    current.last_error = Some(error.into());
    current.observed_ms = previous.observed_ms;
    if current.path == previous.path {
        current.title = previous.title.clone();
    }
    let kind = if error.contains("timed out") {
        std::io::ErrorKind::TimedOut
    } else {
        std::io::ErrorKind::Other
    };
    current.facts.push(super::Fact::failure(
        "Collection completion",
        current.path.clone(),
        std::io::Error::new(kind, error),
    ));
    current.retain_failed_values(previous);
    Some(Arc::new(current))
}

fn observe(
    path: &Path,
    token: &AtomicBool,
    policy: RefreshPolicy,
) -> Result<Arc<Document>, String> {
    observe_with(path, token, policy, super::worker::collect)
}
fn observe_with(
    path: &Path,
    token: &AtomicBool,
    policy: RefreshPolicy,
    collector: impl FnOnce(&Path, &AtomicBool, Option<&Document>) -> Result<Document, String>,
) -> Result<Arc<Document>, String> {
    let (key, interface) = request_key(path)?;
    let fingerprint = observation_identity(&interface).to_vec();
    let entered = Instant::now();
    let (lock, changed) = &*STATE;
    let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if token.load(Ordering::Relaxed) {
            return Err("System observation cancelled".into());
        }
        if let Some(snapshot) = state
            .ready
            .get(&key)
            .filter(|s| valid(s, &fingerprint, entered, policy))
        {
            return Ok(compose(interface, &snapshot.document));
        }
        if !state.loading.contains(&key) {
            break;
        }
        if entered.elapsed() > Duration::from_secs(2) {
            return Err("Shared observation timed out".into());
        }
        state = changed
            .wait_timeout(state, Duration::from_millis(20))
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    state.loading.insert(key.clone());
    let previous = state
        .ready
        .get(&key)
        .map(|snapshot| snapshot.document.clone());
    let seed = state
        .ready
        .get(&key)
        .filter(|snapshot| {
            policy != RefreshPolicy::Manual && reusable_static(snapshot, &fingerprint)
        })
        .map(|snapshot| snapshot.document.clone());
    drop(state);
    let result = collector(path, token, seed.as_deref()).and_then(|mut document| {
        if !super::identity::same_interface(&document, &interface)
            || !super::identity::same_observation(&document, &interface)
        {
            Err("Object changed after request admission".into())
        } else {
            if let Some(previous) = &previous {
                document.retain_failed_values(previous);
            }
            Ok(Arc::new(document))
        }
    });
    let result = result.or_else(|error| {
        let retained = if token.load(Ordering::Relaxed) {
            None
        } else {
            previous.as_ref().and_then(|old| {
                request_key(path)
                    .ok()
                    .and_then(|(_, current)| retained_failure(current, old, &error))
            })
        };
        retained.ok_or(error)
    });
    let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
    state.loading.remove(&key);
    let result = if token.load(Ordering::Relaxed) {
        Err("System observation cancelled".into())
    } else {
        result
    };
    if let Ok(document) = &result {
        state.ready.insert(
            key,
            Snapshot {
                document: document.clone(),
                received: Instant::now(),
                policy,
            },
        );
        while state.ready.len() > 256 {
            let oldest = state
                .ready
                .iter()
                .min_by_key(|(_, snapshot)| snapshot.received)
                .map(|(key, _)| key.clone());
            if let Some(oldest) = oldest {
                state.ready.remove(&oldest);
            }
        }
    }
    changed.notify_all();
    result.map(|observation| compose(interface, &observation))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_manager::system::{Relation, identity::Namespace, model::Fact};
    use little_clip_core::kernel_interface::{Filesystem, Kind};
    use std::path::PathBuf;

    #[test]
    fn cancelled_collector_cannot_publish_a_successful_snapshot_or_strand_loading() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("source");
        std::fs::write(&path, b"metadata fixture").unwrap();
        let token = AtomicBool::new(false);
        let (key, _) = request_key(&path).unwrap();
        let cancelled_completion =
            |path: &Path, token: &AtomicBool, _: Option<&Document>| -> Result<Document, String> {
                let mut document = super::super::worker::interface(path)?;
                document.title = "Cancelled metadata".into();
                document.event_revision = super::super::events::revision();
                super::super::identity::stamp(&mut document);
                token.store(true, Ordering::Relaxed);
                Ok(document)
            };
        let rejected = observe_with(&path, &token, RefreshPolicy::Manual, cancelled_completion);
        assert!(rejected.unwrap_err().contains("cancelled"));
        {
            let state = STATE.0.lock().unwrap();
            assert!(!state.ready.contains_key(&key));
            assert!(!state.loading.contains(&key));
        }
        token.store(false, Ordering::Relaxed);
        let accepted = observe_with(&path, &token, RefreshPolicy::Manual, |path, _, _| {
            let mut document = super::super::worker::interface(path)?;
            document.title = "Accepted metadata".into();
            document.event_revision = super::super::events::revision();
            super::super::identity::stamp(&mut document);
            Ok(document)
        })
        .unwrap();
        assert_eq!(accepted.path, path);
        let published = {
            let state = STATE.0.lock().unwrap();
            assert!(!state.loading.contains(&key));
            state.ready[&key].document.clone()
        };
        assert!(observe_with(&path, &token, RefreshPolicy::Manual, cancelled_completion).is_err());
        {
            let state = STATE.0.lock().unwrap();
            assert!(Arc::ptr_eq(&state.ready[&key].document, &published));
            assert!(!state.loading.contains(&key));
        }
        token.store(false, Ordering::Relaxed);
        let alias = root.path().join("alias");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        let reused = observe_with(&alias, &token, RefreshPolicy::Row, |_, _, _| {
            panic!("the last accepted snapshot must remain reusable")
        })
        .unwrap();
        assert_eq!(reused.title, "Accepted metadata");
    }

    #[test]
    fn cache_preserves_complete_failure_without_presenting_a_new_successful_observation() {
        // Each test thread has a distinct proc object; this reads metadata only.
        let path = PathBuf::from(format!("/proc/self/task/{}/stat", unsafe {
            libc::gettid()
        }));
        let token = AtomicBool::new(false);
        let first = observe_with(&path, &token, RefreshPolicy::Manual, |path, _, _| {
            let mut document = super::super::worker::interface(path)?;
            document.event_revision = super::super::events::revision();
            document.observed_ms = 100;
            document.facts.push(Fact::text(
                "Snapshot",
                "captured value",
                document.path.clone(),
            ));
            super::super::identity::stamp(&mut document);
            Ok(document)
        })
        .unwrap();
        assert_ne!(first.generation, 0);
        assert!(
            first
                .facts
                .iter()
                .all(|fact| fact.observed_generation == Some(first.generation))
        );
        let failed = observe_with(&path, &token, RefreshPolicy::Manual, |_, _, seed| {
            assert!(seed.is_none());
            Err("Collector timed out".into())
        })
        .unwrap();
        assert_eq!(failed.observed_ms, first.observed_ms);
        assert_eq!(failed.generation, first.generation);
        let fact = failed
            .facts
            .iter()
            .find(|fact| fact.label == "Snapshot")
            .unwrap();
        assert!(fact.retained);
        assert_eq!(fact.observed_generation, Some(first.generation));
        assert_eq!(fact.value.as_deref(), Some("captured value"));
        assert_eq!(fact.outcome, super::super::model::Outcome::TimedOut);
        let cached = observe_with(&path, &token, RefreshPolicy::Row, |_, _, _| {
            panic!("a bounded fresh failure snapshot should be shared")
        })
        .unwrap();
        assert_eq!(cached.last_error, failed.last_error);
        assert!(
            cached
                .facts
                .iter()
                .all(|fact| fact.observed_generation == Some(first.generation))
        );
        let (key, _) = request_key(&path).unwrap();
        STATE.0.lock().unwrap().ready.remove(&key);
    }

    fn retained_fixture() -> (tempfile::TempDir, Document, Document) {
        let root = tempfile::tempdir().unwrap();
        let mut previous = super::super::worker::interface(Path::new("/proc/uptime")).unwrap();
        previous.path = root.path().join("observation");
        previous.event_revision = super::super::events::revision();
        previous.observed_ms = 100;
        super::super::identity::stamp(&mut previous);
        let current = previous.clone();
        let mut fact = Fact::text("Temperature", "42 °C", previous.path.join("temp1_input"));
        fact.origin = "documented sysfs attribute".into();
        fact.observed_ms = 73;
        previous.facts.push(fact);
        (root, previous, current)
    }

    #[test]
    fn complete_failure_retains_last_value_and_age_with_the_latest_failure_status() {
        let (_root, previous, current) = retained_fixture();
        let retained = retained_failure(current, &previous, "Collector timed out").unwrap();
        let fact = retained
            .facts
            .iter()
            .find(|fact| fact.label == "Temperature")
            .unwrap();
        assert_eq!(fact.value.as_deref(), Some("42 °C"));
        assert_eq!(fact.observed_ms, 73);
        assert!(fact.retained);
        assert_eq!(fact.outcome, super::super::model::Outcome::TimedOut);
        assert_eq!(retained.observed_ms, 100);
        assert_eq!(retained.generation, previous.generation);
        assert_eq!(retained.last_error.as_deref(), Some("Collector timed out"));
        assert!(retained.summary().contains("last observation"));
    }

    #[test]
    fn complete_failure_does_not_retain_across_changed_or_unknown_namespace_or_identity() {
        let (_root, previous, current) = retained_fixture();
        for variant in 0..5 {
            let mut changed = current.clone();
            match variant {
                0 => changed.namespace.network = None,
                1 => changed.namespace.network = Some(changed.namespace.network.unwrap() + 1),
                2 => changed.namespace.cgroup = None,
                3 => changed.namespace.cgroup = Some(changed.namespace.cgroup.unwrap() + 1),
                _ => changed.identity[1] += 1,
            }
            assert!(retained_failure(changed, &previous, "Collector failed").is_none());
        }
    }

    #[test]
    fn complete_failure_does_not_guess_continuity_when_disk_sequence_cannot_be_revalidated() {
        let (_root, mut previous, current) = retained_fixture();
        previous.facts.push(Fact::text(
            "Disk sequence",
            "10",
            previous.path.join("diskseq"),
        ));
        super::super::identity::stamp(&mut previous);
        assert!(retained_failure(current, &previous, "Collector failed").is_none());
    }

    fn policy_snapshot(path: &str, received: Instant, policy: RefreshPolicy) -> Snapshot {
        let mut document = interface(path, Kind::Character { major: 1, minor: 3 }, false);
        document.kernel_identity = None;
        document.event_revision = super::super::events::revision();
        Snapshot {
            document: Arc::new(document),
            received,
            policy,
        }
    }

    #[test]
    fn automatic_inspector_reads_expired_measurements_before_row_cache_expires() {
        let entered = Instant::now();
        let snapshot = policy_snapshot(
            "/fixture/policy",
            entered - Duration::from_secs(2),
            RefreshPolicy::Row,
        );
        let fingerprint = snapshot.document.identity.clone();
        assert!(reusable_static(&snapshot, &fingerprint));
        assert!(valid(&snapshot, &fingerprint, entered, RefreshPolicy::Row));
        assert!(!valid(
            &snapshot,
            &fingerprint,
            entered,
            RefreshPolicy::Inspector
        ));
        assert!(!valid(
            &snapshot,
            &fingerprint,
            entered,
            RefreshPolicy::Manual
        ));
    }

    #[test]
    fn manual_refresh_does_not_coalesce_with_a_stable_seeded_automatic_completion() {
        let entered = Instant::now();
        let mut snapshot = policy_snapshot(
            "/fixture/coalescing",
            entered + Duration::from_millis(1),
            RefreshPolicy::Inspector,
        );
        let fingerprint = snapshot.document.identity.clone();
        assert!(valid(
            &snapshot,
            &fingerprint,
            entered,
            RefreshPolicy::Inspector
        ));
        assert!(!valid(
            &snapshot,
            &fingerprint,
            entered,
            RefreshPolicy::Manual
        ));
        snapshot.policy = RefreshPolicy::Manual;
        assert!(valid(
            &snapshot,
            &fingerprint,
            entered,
            RefreshPolicy::Manual
        ));
        assert!(valid(&snapshot, &fingerprint, entered, RefreshPolicy::Row));
    }

    fn interface(path: &str, kind: Kind, directory: bool) -> Document {
        Document {
            path: path.into(),
            interface_directory: directory,
            identity: vec![1, 2, 3, 4, 5],
            namespace: Namespace {
                mount: Some(1),
                pid: Some(2),
                network: Some(3),
                cgroup: Some(4),
            },
            kernel_identity: Some(vec![10, 20, 0, 30, 40]),
            interface_context: Some(super::super::identity::ObjectContext {
                object: super::super::identity::MountIdentity {
                    device: 1,
                    inode: 2,
                    mount: 100,
                },
                kind,
                file_type: if directory {
                    libc::S_IFDIR
                } else {
                    libc::S_IFREG
                },
            }),
            kernel_context: Some(super::super::identity::ObjectContext {
                object: super::super::identity::MountIdentity {
                    device: 10,
                    inode: 20,
                    mount: 200,
                },
                kind: Kind::Kernel {
                    filesystem: Filesystem::Sysfs,
                },
                file_type: libc::S_IFDIR,
            }),
            generation: 42,
            event_revision: 0,
            last_error: None,
            kind,
            title: "Interface".into(),
            observed_ms: 100,
            facts: vec![],
            storage: None,
            topology: None,
            cpu: None,
            telemetry: None,
            numa: None,
            relations: vec![Relation {
                label: "Kernel object".into(),
                target: "/sys/devices/fixture".into(),
                identity: Some(vec![10, 20, 0, 30, 40]),
                context: None,
                directory: true,
                system: true,
                process_start: None,
                loop_association: None,
                cgroup_hierarchy: None,
            }],
        }
    }

    #[test]
    fn unknown_mount_context_cannot_reuse_even_a_fresh_snapshot() {
        let entered = Instant::now();
        let mut snapshot = policy_snapshot("/fixture/unknown-context", entered, RefreshPolicy::Row);
        snapshot.document = Arc::new({
            let mut document = (*snapshot.document).clone();
            document.interface_context = None;
            document
        });
        assert!(!valid(
            &snapshot,
            &snapshot.document.identity,
            entered,
            RefreshPolicy::Row
        ));
    }

    #[test]
    fn changed_mount_context_completion_is_rejected_despite_unchanged_inode() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("backing");
        std::fs::write(&path, "ordinary backing data").unwrap();
        let result = observe_with(
            &path,
            &AtomicBool::new(false),
            RefreshPolicy::Manual,
            |path, _, _| {
                let mut document = super::super::worker::interface(path)?;
                document.interface_context.as_mut().unwrap().object.mount += 1;
                Ok(document)
            },
        );
        assert!(
            result
                .unwrap_err()
                .contains("Object changed after request admission")
        );
    }

    #[test]
    fn shared_cpu_observations_keep_low_level_access_roles_separate() {
        use little_clip_core::kernel_interface::{Filesystem, Kind};
        let mut access = interface(
            "/dev/cpu/0/cpuid",
            Kind::Character {
                major: 203,
                minor: 0,
            },
            false,
        );
        access.title = "CPU CPUID access interface".into();
        access.cpu = Some(super::super::cpu::Cpu {
            role: "Logical CPU topology".into(),
            summary: "cpu0".into(),
            bins: Vec::new(),
            memberships: Vec::new(),
            focus: None,
            coverage: super::super::model::Outcome::Observed,
            retained: false,
        });
        let object = interface(
            "/sys/devices/fixture",
            Kind::Kernel {
                filesystem: Filesystem::Sysfs,
            },
            true,
        );
        let object = compose(object, &access);
        assert_eq!(object.title, "Logical CPU topology");
        let access = compose(access, &object);
        assert_eq!(access.title, "CPU CPUID access interface");
    }
    #[test]
    fn dev_and_sys_share_object_identity_but_attributes_are_independent() {
        let node = interface(
            "/dev/fixture",
            Kind::Character {
                major: 240,
                minor: 0,
            },
            false,
        );
        let object = interface(
            "/sys/devices/fixture",
            Kind::Kernel {
                filesystem: Filesystem::Sysfs,
            },
            true,
        );
        let attribute = interface(
            "/sys/devices/fixture/value",
            Kind::Kernel {
                filesystem: Filesystem::Sysfs,
            },
            false,
        );
        assert_eq!(node.shared_object(), object.shared_object());
        assert_eq!(observation_identity(&node), observation_identity(&object));
        assert!(attribute.shared_object().is_none());
        assert_eq!(observation_identity(&attribute), attribute.identity);
    }

    #[test]
    fn shared_observation_preserves_new_interface_metadata_and_source_provenance() {
        let mut object = interface(
            "/sys/devices/fixture",
            Kind::Kernel {
                filesystem: Filesystem::Sysfs,
            },
            true,
        );
        let mut sys_permissions = Fact::text("Permissions", "0444", object.path.clone());
        sys_permissions.interface = true;
        object.facts.push(sys_permissions);
        object.facts.push(Fact::text(
            "Product",
            "Example device",
            object.path.join("product"),
        ));
        let mut node = interface(
            "/dev/fixture",
            Kind::Character {
                major: 240,
                minor: 0,
            },
            false,
        );
        node.identity[1] = 999;
        let mut permissions = Fact::text("Permissions", "0600", node.path.clone());
        permissions.interface = true;
        node.facts.push(permissions);
        let combined = compose(node, &object);
        assert_eq!(combined.path, Path::new("/dev/fixture"));
        assert_eq!(combined.identity[1], 999);
        assert_eq!(
            combined
                .facts
                .iter()
                .filter(|fact| fact.label == "Permissions")
                .count(),
            1
        );
        assert_eq!(
            combined
                .facts
                .iter()
                .find(|fact| fact.label == "Permissions")
                .unwrap()
                .value
                .as_deref(),
            Some("0600")
        );
        assert_eq!(
            combined
                .facts
                .iter()
                .find(|fact| fact.label == "Product")
                .unwrap()
                .source,
            Path::new("/sys/devices/fixture/product")
        );
        let mut replacement = (*combined).clone();
        replacement.identity[1] = 1000;
        let recomposed = compose(replacement, &combined);
        assert_eq!(recomposed.identity[1], 1000);
    }

    #[test]
    fn failed_refresh_preserves_value_age_and_error_without_attaching_to_replacements() {
        use crate::file_manager::system::model::Outcome;
        let mut previous = interface(
            "/sys/devices/fixture",
            Kind::Kernel {
                filesystem: Filesystem::Sysfs,
            },
            true,
        );
        let mut temperature = Fact::text("Temperature", "42 °C", previous.path.join("temp1_input"));
        temperature.origin = "documented sysfs attribute".into();
        temperature.observed_ms = 100;
        previous.facts.push(temperature);
        let mut refresh = previous.clone();
        refresh.observed_ms = 200;
        refresh.facts.clear();
        let mut denied = Fact::failure(
            "Temperature",
            previous.path.join("temp1_input"),
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "Access denied"),
        );
        denied.origin = "documented sysfs attribute".into();
        refresh.facts.push(denied);
        let mut replacement = refresh.clone();
        replacement.generation += 1;
        replacement.retain_failed_values(&previous);
        assert!(replacement.facts[0].value.is_none());
        refresh.retain_failed_values(&previous);
        let fact = &refresh.facts[0];
        assert_eq!(fact.value.as_deref(), Some("42 °C"));
        assert_eq!(fact.outcome, Outcome::PermissionDenied);
        assert_eq!(fact.observed_ms, 100);
        assert!(fact.retained);
        assert!(fact.error.as_ref().unwrap().contains("Access denied"));
        assert!(refresh.summary().contains("last observation"));
        let mut partial = previous.clone();
        partial.facts.clear();
        partial.last_error = Some("Collection timed out".into());
        partial.facts.push(Fact::failure(
            "Collection completion",
            partial.path.clone(),
            std::io::Error::new(std::io::ErrorKind::TimedOut, "Collection timed out"),
        ));
        partial.retain_failed_values(&previous);
        assert!(partial.facts.iter().any(|fact| fact.label == "Temperature"
            && fact.retained
            && fact.outcome == Outcome::TimedOut));
    }
}
