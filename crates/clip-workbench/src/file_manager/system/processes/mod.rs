//! Shared, throttled process observations. FD targets are pinned for metadata only.
mod scan;
mod stat;
use super::{Document, Fact, Relation, identity::Namespace, model::Outcome, worker};
use crate::file_manager::preview_job::Task;
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, LazyLock, Mutex, atomic::AtomicBool},
    time::{Duration, Instant},
};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Device {
    #[serde(default)]
    character: bool,
    rdev: u64,
    // Missing lifetime metadata from older helper records cannot prove access.
    #[serde(default)]
    identity: Vec<u64>,
}
impl Device {
    fn predates(&self, observed_ms: u64) -> bool {
        // The shared fingerprint is dev/inode/rdev/ctime/ctime_nsec. Unknown
        // layouts and a node changed in the scan's starting millisecond are
        // conservative exclusions until the next shared scan.
        let [_, _, _, seconds, nanos] = self.identity.as_slice() else {
            return false;
        };
        *nanos < 1_000_000_000
            && seconds
                .checked_mul(1000)
                .and_then(|time| time.checked_add(*nanos / 1_000_000))
                .is_some_and(|changed| changed < observed_ms)
    }
    fn from_metadata(metadata: &std::fs::Metadata) -> Option<Self> {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let character = metadata.file_type().is_char_device();
        (character || metadata.file_type().is_block_device()).then(|| Self {
            character,
            rdev: metadata.rdev(),
            identity: super::identity::fingerprint(metadata),
        })
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Process {
    stat: stat::Stat,
    devices: Vec<Device>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Coverage {
    pids: usize,
    fds: usize,
    denied: usize,
    vanished: usize,
    malformed: usize,
    other: usize,
    truncated: bool,
    timed_out: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Snapshot {
    namespace: Namespace,
    observed_ms: u64,
    elapsed_ms: u64,
    coverage: Coverage,
    processes: Vec<Process>,
}
impl Snapshot {
    fn new() -> Self {
        Self {
            namespace: Namespace::current(),
            observed_ms: super::model::now_ms(),
            elapsed_ms: 0,
            coverage: Coverage::default(),
            processes: Vec::new(),
        }
    }
}
struct Cache {
    namespace: Namespace,
    event_revision: u64,
    ready: Option<Arc<Snapshot>>,
    pending: Option<Task<Result<Snapshot, String>>>,
    requested: Option<Instant>,
    error: Option<Fact>,
}
static JOBS: LazyLock<little_clip_core::BackgroundJobs> =
    LazyLock::new(|| little_clip_core::BackgroundJobs::new(1, 1));
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| {
    Mutex::new(Cache {
        namespace: Namespace::current(),
        event_revision: super::events::revision(),
        ready: None,
        pending: None,
        requested: None,
        error: None,
    })
});
const REFRESH: Duration = Duration::from_secs(5);
fn snapshot(namespace: &Namespace) -> (Option<Arc<Snapshot>>, Option<Fact>) {
    let mut state = CACHE.lock().unwrap_or_else(|error| error.into_inner());
    let revision = super::events::revision();
    let invalidated = super::events::affected(state.event_revision, [Path::new("/sys")]);
    let result = snapshot_in(&mut state, namespace, invalidated);
    state.event_revision = revision;
    result
}
fn snapshot_in(
    state: &mut Cache,
    namespace: &Namespace,
    invalidated: bool,
) -> (Option<Arc<Snapshot>>, Option<Fact>) {
    let namespace_changed = state.namespace != *namespace;
    if namespace_changed || invalidated || !namespace.known() {
        state.pending = None;
        state.ready = None;
        state.error = None;
        if namespace_changed {
            state.requested = None;
            state.namespace = namespace.clone();
        }
    }
    if !namespace.known() {
        return (None, Some(scan_failure("namespace context unknown".into())));
    }
    if let Some(result) = state.pending.as_ref().and_then(Task::poll) {
        state.pending = None;
        match result
            .map_err(|error| error.to_string())
            .and_then(|result| result)
        {
            Ok(snapshot) if snapshot.namespace == *namespace => {
                state.ready = Some(Arc::new(snapshot));
                state.error = None;
            }
            error => {
                state.error =
                    Some(scan_failure(error.err().unwrap_or_else(|| {
                        "Namespace changed during process scan".into()
                    })));
            }
        }
    }
    if state.pending.is_none() && state.requested.is_none_or(|time| time.elapsed() >= REFRESH) {
        state.requested = Some(Instant::now());
        state.pending = Some(
            // This is an asynchronous shared cache: consume completed scans even
            // when the next visible row polls after its own refresh interval.
            // The owned helper still enforces its independent wall deadline.
            Task::start(&JOBS, super::RESPONSE_DEADLINE, collect),
        );
    }
    let ready = state
        .ready
        .clone()
        .filter(|snapshot| super::model::now_ms().saturating_sub(snapshot.observed_ms) <= 10000);
    (ready, state.error.clone())
}
fn scan_failure(error: String) -> Fact {
    let kind = if error.contains("timed out") {
        io::ErrorKind::TimedOut
    } else {
        io::ErrorKind::Other
    };
    let message = format!("Process scan unavailable: {error}");
    let mut fact = Fact::failure(
        "Process scan refresh",
        "/proc".into(),
        io::Error::new(kind, error),
    );
    fact.value = Some(message);
    fact.origin = "shared bounded process scan".into();
    fact
}

fn collect(token: &AtomicBool) -> Result<Snapshot, String> {
    let directory = tempfile::Builder::new()
        .prefix("veylar-process-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let output = directory.path().join("result.json");
    let mut command = Command::new("/proc/self/exe");
    command
        .arg("--internal-system-processes")
        .arg(&output)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let id = worker::spawn_command(command)?;
    worker::wait(id, token)?;
    let bytes = crate::file_source::read_file_bytes(&output, 1024 * 1024, true, false)
        .map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}
pub(in crate::file_manager) fn run(output: &Path) -> Result<(), String> {
    let snapshot = scan::collect(Path::new("/proc")).map_err(|error| error.to_string())?;
    if snapshot.namespace != Namespace::current() {
        return Err("Namespace changed during process scan".into());
    }
    worker::write_output(output, &snapshot)
}
fn device(path: &Path) -> Option<Device> {
    let file = little_clip_core::kernel_interface::pin(path, true).ok()?;
    Device::from_metadata(&file.metadata().ok()?)
}

fn device_targets(document: &Document) -> Vec<Device> {
    let mut devices = Vec::new();
    if let Some(device) = device(&document.path) {
        // Static pseudo-devices do not benefit from a process inventory.
        if libc::major(device.rdev) == 1 {
            return Vec::new();
        }
        devices.push(device);
    } else {
        for relation in document.relations.iter().take(64) {
            if let Some(device) = device(&relation.target) {
                if !devices.contains(&device) {
                    devices.push(device);
                }
            }
        }
    }
    devices
}
pub(super) fn attach(document: &mut Document) {
    if document.namespace != Namespace::current() {
        return;
    }
    let devices = device_targets(document);
    let active_vt = super::terminal::active_vt(document);
    if devices.is_empty() && active_vt.is_none() {
        return;
    }
    let (snapshot, error) = snapshot(&document.namespace);
    let Some(snapshot) = snapshot else {
        let fact = error.unwrap_or_else(|| {
            let mut fact = Fact::text(
                "Process associations",
                "Shared scan pending; no conclusion about open descriptors",
                "/proc".into(),
            );
            fact.outcome = Outcome::NotRequested;
            fact
        });
        document.facts.push(fact);
        return;
    };
    append_snapshot(document, &snapshot, devices, active_vt, error);
}
fn append_snapshot(
    document: &mut Document,
    snapshot: &Snapshot,
    mut devices: Vec<Device>,
    active_vt: Option<u8>,
    error: Option<Fact>,
) {
    let had_devices = !devices.is_empty();
    devices.retain(|device| device.predates(snapshot.observed_ms));
    if had_devices && devices.is_empty() && active_vt.is_none() {
        let mut fact = Fact::text(
            "Process associations",
            "Shared scan predates this node's metadata lifetime; waiting for a fresh scan",
            document.path.clone(),
        );
        fact.outcome = Outcome::NotRequested;
        document.facts.push(fact);
        document.facts.extend(error);
        return;
    }
    let mut matched = 0;
    let mut rejected = 0;
    let mut terminal = Vec::new();
    let mut possible_terminal = Vec::new();
    for process in &snapshot.processes {
        let Some(association) = association(&devices, process, active_vt) else {
            continue;
        };
        let path = PathBuf::from(format!("/proc/{}", process.stat.pid));
        matched += 1;
        if matched <= 32 {
            let reason = match association {
                Association::Descriptor => "observed open descriptor",
                Association::VirtualConsole => "observed controlling virtual console",
                Association::PossiblePty => {
                    "possible controlling terminal; devpts instance unverified"
                }
            };
            let mut relation = Relation::new(
                format!(
                    "{} (PID {}) · {reason}",
                    process.stat.name, process.stat.pid
                ) + if error.is_some() {
                    " · retained snapshot; refresh failed"
                } else {
                    ""
                },
                path,
            );
            relation.process_start = Some(process.stat.start);
            if relation.identity.is_none() {
                rejected += 1;
            }
            document.relations.push(relation);
            if active_vt.is_some_and(|number| process.stat.tty == (4 << 8) | u32::from(number)) {
                terminal.push(process);
            } else if let Some(device) = devices
                .iter()
                .find(|device| tty_matches(device, &process.stat))
            {
                // An exact open FD proves access to this node, but tty_nr still
                // cannot identify which devpts instance is controlling the process.
                if (136..=143).contains(&libc::major(device.rdev)) {
                    possible_terminal.push(process);
                } else {
                    terminal.push(process);
                }
            }
        }
    }
    terminal_facts(
        document,
        &possible_terminal,
        snapshot.observed_ms,
        false,
        error.as_ref(),
    );
    terminal_facts(
        document,
        &terminal,
        snapshot.observed_ms,
        true,
        error.as_ref(),
    );
    append_coverage(document, snapshot, matched, rejected, error.as_ref());
    document.facts.extend(error);
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Association {
    Descriptor,
    VirtualConsole,
    PossiblePty,
}
fn association(
    devices: &[Device],
    process: &Process,
    active_vt: Option<u8>,
) -> Option<Association> {
    if process
        .devices
        .iter()
        .any(|device| !device.identity.is_empty() && devices.contains(device))
    {
        return Some(Association::Descriptor);
    }
    if active_vt.is_some_and(|number| process.stat.tty == (4 << 8) | u32::from(number)) {
        return Some(Association::VirtualConsole);
    }
    devices.iter().find_map(|device| {
        if !tty_matches(device, &process.stat) {
            return None;
        }
        match (libc::major(device.rdev), libc::minor(device.rdev)) {
            (4, 1..=63) => Some(Association::VirtualConsole),
            (136..=143, _) => Some(Association::PossiblePty),
            _ => None,
        }
    })
}
impl Coverage {
    fn outcome(&self, matched: usize, rejected: usize) -> Outcome {
        let coverage = self;
        if coverage.timed_out {
            Outcome::TimedOut
        } else if coverage.truncated || matched > 32 {
            Outcome::Truncated
        } else if coverage.denied > 0 {
            Outcome::PermissionDenied
        } else if coverage.vanished > 0 || rejected > 0 {
            Outcome::Disappeared
        } else if coverage.malformed > 0 {
            Outcome::Malformed
        } else if coverage.other > 0 {
            Outcome::Unsupported
        } else {
            Outcome::Observed
        }
    }
}
fn append_coverage(
    document: &mut Document,
    snapshot: &Snapshot,
    matched: usize,
    rejected: usize,
    error: Option<&Fact>,
) {
    let coverage = &snapshot.coverage;
    let status = coverage.outcome(matched, rejected);
    let label = if error.is_some() {
        format!("Previous scan: {status}")
    } else {
        status.to_string()
    };
    let mut fact = Fact::text(
        "Process scan coverage",
        format!(
            "{label} · {matched} associated or candidate accessible processes; {} PIDs / {} FDs visited; {} permission gaps, {} vanished, {} malformed, {} other errors, {rejected} inaccessible process targets; {} ms. FD presence does not establish active use or exclusive ownership.",
            coverage.pids,
            coverage.fds,
            coverage.denied,
            coverage.vanished,
            coverage.malformed,
            coverage.other,
            snapshot.elapsed_ms
        ),
        PathBuf::from("/proc"),
    );
    fact.observed_ms = snapshot.observed_ms;
    fact.origin = "shared bounded process scan".into();
    fact.outcome = status;
    if let Some(error) = error {
        fact.error = error.error.clone();
        fact.outcome = error.outcome.clone();
        fact.retained = true;
    }
    document.facts.push(fact);
}
fn tty_matches(device: &Device, stat: &stat::Stat) -> bool {
    let major = (stat.tty >> 8) & 255;
    let minor = (stat.tty & 255) | ((stat.tty >> 12) & 0xfff00);
    device.character
        && stat.tty != 0
        && libc::major(device.rdev) == major
        && libc::minor(device.rdev) == minor
}
fn terminal_facts(
    document: &mut Document,
    processes: &[&Process],
    observed_ms: u64,
    verified: bool,
    failure: Option<&Fact>,
) {
    if processes.is_empty() {
        return;
    }
    let directory_inventory = document.interface_directory
        && matches!(
            document.kind,
            little_clip_core::kernel_interface::Kind::Kernel {
                filesystem: little_clip_core::kernel_interface::Filesystem::Devpts
                    | little_clip_core::kernel_interface::Filesystem::DeviceNodes,
            }
        );
    for process in processes.iter().rev() {
        let stat = &process.stat;
        let mut value = format!(
            "PID {} ({}) · start identity {} · session {} · group {} · foreground group {}{}{}",
            stat.pid,
            stat.name,
            stat.start,
            stat.session,
            stat.group,
            stat.foreground,
            if stat.group == stat.foreground && stat.foreground > 0 {
                " · foreground group member"
            } else {
                ""
            },
            if stat.pid as i64 == stat.session as i64 {
                " · session leader (not necessarily a shell)"
            } else {
                ""
            }
        );
        if !verified {
            value.push_str(" · Candidate only: devpts instance unverified; device number alone does not identify this terminal");
        }
        let label = if verified {
            "Observed terminal relationships"
        } else {
            "Possible terminal relationships"
        };
        let mut fact = Fact::text(
            format!("{label} · PID {}", stat.pid),
            value,
            PathBuf::from(format!("/proc/{}/stat", stat.pid)),
        );
        fact.observed_ms = observed_ms;
        fact.origin = "shared bounded process scan · stat fields".into();
        if let Some(failure) = failure {
            fact.retained = true;
            fact.outcome = failure.outcome.clone();
            fact.error = failure.error.clone();
        }
        if directory_inventory {
            document.facts.push(fact);
        } else {
            document.facts.insert(0, fact);
        }
    }
    if matches!(
        document.kind,
        little_clip_core::kernel_interface::Kind::Character { .. }
    ) {
        document.title = "Terminal / session interface".into();
    }
}
pub(super) fn describe(document: &mut Document) {
    let path = &document.path;
    if path.parent() != Some(Path::new("/proc"))
        || path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse::<u32>().ok())
            .is_none()
    {
        return;
    }
    match stat::read(path) {
        Ok(stat) => {
            document.title = format!("Process {} · {}", stat.pid, stat.name);
            for (label, value) in [
                (
                    "Process identity",
                    format!(
                        "PID {} · start tick {} in this procfs view",
                        stat.pid, stat.start
                    ),
                ),
                ("Process start tick", stat.start.to_string()),
                ("Parent PID", stat.parent.to_string()),
                ("Session", stat.session.to_string()),
                ("Process group", stat.group.to_string()),
                ("Controlling terminal number", stat.tty.to_string()),
                ("Foreground group", stat.foreground.to_string()),
            ] {
                document
                    .facts
                    .push(Fact::text(label, value, path.join("stat")));
            }
        }
        Err(error) => {
            document
                .facts
                .push(Fact::failure("Process identity", path.join("stat"), error))
        }
    }
}
pub(super) fn shutdown() {
    CACHE
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .pending = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn event_invalidation_retires_cached_and_pending_process_scans_without_bypassing_throttle() {
        use std::sync::{atomic::Ordering, mpsc};
        let snapshot = Snapshot::new();
        let namespace = snapshot.namespace.clone();
        let jobs = little_clip_core::BackgroundJobs::new(1, 1);
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let old = snapshot.clone();
        let pending = Task::start(&jobs, Duration::MAX, move |_| {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            finished_tx.send(()).unwrap();
            Ok(old)
        });
        let token = pending.token.clone();
        started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let requested = Instant::now();
        let mut cache = Cache {
            namespace: namespace.clone(),
            event_revision: 7,
            ready: Some(Arc::new(snapshot)),
            pending: Some(pending),
            requested: Some(requested),
            error: None,
        };
        let result = snapshot_in(&mut cache, &namespace, true);
        // Release owned work before assertions, including in the pre-fix failure.
        release_tx.send(()).unwrap();
        finished_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(
            result.0.is_none(),
            "invalidated process associations must not be reused"
        );
        assert!(cache.ready.is_none());
        assert!(cache.pending.is_none());
        assert!(token.load(Ordering::Relaxed));
        assert_eq!(cache.requested, Some(requested));
    }

    #[test]
    fn unknown_namespace_retires_process_cache_without_admitting_a_scan() {
        let snapshot = Snapshot::new();
        let mut namespace = snapshot.namespace.clone();
        namespace.cgroup = None;
        let mut cache = Cache {
            namespace: namespace.clone(),
            event_revision: 0,
            ready: Some(Arc::new(snapshot)),
            pending: None,
            requested: None,
            error: None,
        };
        let result = snapshot_in(&mut cache, &namespace, false);
        assert!(result.0.is_none());
        assert!(
            result
                .1
                .unwrap()
                .error
                .unwrap()
                .contains("namespace context unknown")
        );
        assert!(cache.ready.is_none());
        assert!(cache.pending.is_none());
        assert!(cache.requested.is_none());
    }

    #[test]
    fn replacement_terminal_cannot_receive_candidates_from_an_older_scan() {
        let (mut document, mut snapshot, selected) = terminal_snapshot_fixture(101, 100_000);
        append_snapshot(&mut document, &snapshot, vec![selected.clone()], None, None);
        assert!(document.relations.is_empty());
        assert!(
            document
                .facts
                .iter()
                .all(|fact| !fact.label.contains("terminal relationships"))
        );
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.outcome == Outcome::NotRequested)
        );
        document.facts.clear();
        snapshot.observed_ms = 102_000;
        append_snapshot(&mut document, &snapshot, vec![selected], None, None);
        assert_eq!(document.relations.len(), 1);
        assert!(
            document
                .facts
                .iter()
                .any(|fact| fact.label.contains("terminal relationships"))
        );
    }

    #[test]
    fn failed_process_refresh_preserves_terminal_age_and_exposes_failure_status() {
        let (mut document, snapshot, selected) = terminal_snapshot_fixture(100, 123_000);
        let mut failure = scan_failure("Process scan timed out".into());
        failure.observed_ms = 200_000;
        append_snapshot(
            &mut document,
            &snapshot,
            vec![selected],
            None,
            Some(failure),
        );
        let terminal = document
            .facts
            .iter()
            .find(|fact| fact.label.contains("terminal relationships"))
            .unwrap();
        assert_eq!(terminal.observed_ms, 123_000);
        assert!(terminal.retained);
        assert_eq!(terminal.outcome, Outcome::TimedOut);
        assert!(terminal.error.as_deref().unwrap().contains("timed out"));
        let coverage = document
            .facts
            .iter()
            .find(|fact| fact.label == "Process scan coverage")
            .unwrap();
        assert_eq!(coverage.observed_ms, 123_000);
        assert!(coverage.retained);
        assert_eq!(coverage.outcome, Outcome::TimedOut);
        assert!(
            coverage
                .value
                .as_deref()
                .unwrap()
                .starts_with("Previous scan:")
        );
        let failure = document
            .facts
            .iter()
            .find(|fact| fact.label == "Process scan refresh")
            .unwrap();
        assert_eq!(failure.observed_ms, 200_000);
        assert!(!failure.retained);
        assert!(
            document.relations[0]
                .label
                .contains("retained snapshot; refresh failed")
        );
    }

    #[test]
    fn device_age_checks_reject_unknown_overflow_and_same_millisecond_lifetimes() {
        let mut device = Device {
            character: true,
            rdev: 1,
            identity: vec![1, 2, 1, 100, 0],
        };
        assert!(device.predates(100_001));
        assert!(!device.predates(100_000));
        device.identity[4] = 1_000_000_000;
        assert!(!device.predates(101_000));
        device.identity[3] = u64::MAX;
        device.identity[4] = 0;
        assert!(!device.predates(u64::MAX));
        device.identity.clear();
        assert!(!device.predates(u64::MAX));
    }

    fn terminal_snapshot_fixture(
        changed_seconds: u64,
        observed_ms: u64,
    ) -> (Document, Snapshot, Device) {
        let mut document = worker::interface(Path::new("/proc/uptime")).unwrap();
        document.facts.clear();
        document.relations.clear();
        let selected = Device {
            character: true,
            rdev: libc::makedev(136, 1),
            identity: vec![10, 11, libc::makedev(136, 1), changed_seconds, 0],
        };
        let mut snapshot = Snapshot::new();
        snapshot.observed_ms = observed_ms;
        snapshot.processes = vec![process_fixture()];
        (document, snapshot, selected)
    }

    fn process_fixture() -> Process {
        Process {
            stat: stat::Stat {
                pid: 42,
                start: 100,
                name: "fixture".into(),
                parent: 1,
                group: 42,
                session: 42,
                tty: 34817,
                foreground: 42,
            },
            devices: Vec::new(),
        }
    }

    #[test]
    fn terminal_associations_do_not_reclassify_the_underlying_usb_object() {
        let mut document = worker::interface(Path::new("/proc/uptime")).unwrap();
        document.title = "USB device".into();
        let process = process_fixture();
        terminal_facts(&mut document, &[&process], 123, true, None);
        assert_eq!(document.title, "USB device");
        assert_eq!(document.facts[0].observed_ms, 123);
        assert_eq!(document.facts[0].source, Path::new("/proc/42/stat"));
        assert!(
            document.facts[0]
                .value
                .as_deref()
                .unwrap()
                .contains("start identity 100")
        );
        document.kind = little_clip_core::kernel_interface::Kind::Character {
            major: 136,
            minor: 1,
        };
        terminal_facts(&mut document, &[&process], 456, true, None);
        assert_eq!(document.title, "Terminal / session interface");
    }
    #[test]
    fn terminal_relationships_keep_each_process_source_and_role() {
        let mut document = worker::interface(Path::new("/proc/uptime")).unwrap();
        document.facts.clear();
        let mut first = process_fixture();
        first.stat.name = "leader".into();
        first.stat.foreground = 43;
        let mut second = Process {
            stat: first.stat.clone(),
            devices: Vec::new(),
        };
        second.stat.pid = 43;
        second.stat.start = 200;
        second.stat.group = 43;
        terminal_facts(&mut document, &[&first, &second], 123, true, None);
        assert_eq!(document.facts.len(), 2);
        for (fact, pid) in document.facts.iter().zip([42, 43]) {
            assert_eq!(fact.source, PathBuf::from(format!("/proc/{pid}/stat")));
            assert_eq!(fact.observed_ms, 123);
        }
        let leader = document.facts[0].value.as_deref().unwrap();
        assert!(leader.contains("session leader (not necessarily a shell)"));
        assert!(!leader.contains("foreground group member"));
        let foreground = document.facts[1].value.as_deref().unwrap();
        assert!(foreground.contains("foreground group member"));
        assert!(!foreground.contains("session leader"));
        first.stat.foreground = -1;
        document.facts.clear();
        terminal_facts(&mut document, &[&first], 456, true, None);
        assert!(
            !document.facts[0]
                .value
                .as_deref()
                .unwrap()
                .contains("foreground group member")
        );
    }
    #[test]
    fn controlling_terminal_matches_without_descriptors_keep_instance_uncertainty() {
        let mut selected = Device {
            character: true,
            rdev: libc::makedev(4, 2),
            identity: vec![10, 11, libc::makedev(4, 2), 100, 0],
        };
        let mut process = process_fixture();
        process.stat.tty = (4 << 8) | 2;
        assert_eq!(
            association(&[selected.clone()], &process, None),
            Some(Association::VirtualConsole)
        );
        selected.character = false;
        assert_eq!(association(&[selected.clone()], &process, None), None);
        selected.character = true;
        selected.rdev = libc::makedev(136, 1);
        selected.identity[2] = selected.rdev;
        process.stat.tty = (136 << 8) | 1;
        assert_eq!(
            association(&[selected.clone()], &process, None),
            Some(Association::PossiblePty)
        );
        let mut other_instance = selected.clone();
        other_instance.identity[0] = 20;
        process.devices.push(other_instance);
        assert_eq!(
            association(&[selected.clone()], &process, None),
            Some(Association::PossiblePty)
        );
        process.devices.push(selected.clone());
        assert_eq!(
            association(&[selected], &process, None),
            Some(Association::Descriptor)
        );
        process.devices.clear();
        process.stat.tty = 0;
        assert_eq!(association(&[], &process, None), None);
    }

    #[test]
    fn active_console_context_matches_only_the_observed_virtual_terminal() {
        let mut process = process_fixture();
        process.stat.tty = (4 << 8) | 2;
        assert_eq!(
            association(&[], &process, Some(2)),
            Some(Association::VirtualConsole)
        );
        assert_eq!(association(&[], &process, Some(1)), None);
        assert_eq!(association(&[], &process, None), None);
        process.stat.tty = (136 << 8) | 2;
        assert_eq!(association(&[], &process, Some(2)), None);
    }

    #[test]
    fn uncertain_pty_relationships_are_never_labeled_observed_terminal_identity() {
        let mut document = worker::interface(Path::new("/proc/uptime")).unwrap();
        document.facts.clear();
        let process = process_fixture();
        terminal_facts(&mut document, &[&process], 123, false, None);
        assert!(
            document.facts[0]
                .label
                .starts_with("Possible terminal relationships")
        );
        assert!(
            document.facts[0]
                .value
                .as_deref()
                .unwrap()
                .contains("devpts instance unverified")
        );
        assert_eq!(document.facts[0].source, Path::new("/proc/42/stat"));
    }

    #[test]
    fn directory_inventory_stays_above_associated_process_facts() {
        let mut document = worker::interface(Path::new("/proc/uptime")).unwrap();
        document.kind = little_clip_core::kernel_interface::Kind::Kernel {
            filesystem: little_clip_core::kernel_interface::Filesystem::Devpts,
        };
        document.interface_directory = true;
        document.facts = vec![Fact::text(
            "Allocated pseudo-terminals",
            "3 observed allocated PTYs",
            "/dev/pts".into(),
        )];
        let process = process_fixture();
        terminal_facts(&mut document, &[&process], 123, false, None);
        assert_eq!(document.facts[0].label, "Allocated pseudo-terminals");
        assert_eq!(document.facts[1].source, Path::new("/proc/42/stat"));
        assert_eq!(document.facts[1].observed_ms, 123);
    }

    #[test]
    fn recycled_device_numbers_and_inodes_cannot_reuse_cached_descriptor_associations() {
        let rdev = libc::makedev(136, 1);
        let descriptor = |changed: u64| -> Device {
            serde_json::from_value(serde_json::json!({
                "character": true, "dev": 10, "inode": 11, "rdev": rdev,
                "identity": [10, 11, rdev, changed, 0],
            }))
            .unwrap()
        };
        let observed = descriptor(100);
        let replacement = descriptor(101);
        let mut process = process_fixture();
        process.stat.tty = 0;
        process.devices = vec![observed.clone()];
        assert_eq!(
            association(&[observed], &process, None),
            Some(Association::Descriptor)
        );
        assert_eq!(association(&[replacement], &process, None), None);
        let legacy: Device = serde_json::from_value(serde_json::json!({
            "character": true, "dev": 10, "inode": 11, "rdev": rdev,
        }))
        .unwrap();
        assert_eq!(
            association(std::slice::from_ref(&legacy), &process, None),
            None
        );
        process.devices = vec![legacy.clone()];
        assert_eq!(association(&[legacy], &process, None), None);
    }

    #[test]
    fn equal_device_numbers_do_not_merge_distinct_devpts_instances() {
        let first = Device {
            character: true,
            rdev: libc::makedev(136, 1),
            identity: vec![10, 11, libc::makedev(136, 1), 100, 0],
        };
        let mut second = first.clone();
        second.identity[0] = 20; // A separate devpts filesystem.
        let mut recycled = first.clone();
        recycled.identity[1] = 12; // A different inode for the same device number.
        assert_ne!(first, second);
        assert_ne!(first, recycled);
        let mut process = process_fixture();
        process.stat.tty = 0;
        process.devices = vec![first.clone()];
        assert_eq!(
            association(&[first], &process, None),
            Some(Association::Descriptor)
        );
        assert_eq!(association(&[second], &process, None), None);
        assert_eq!(association(&[recycled], &process, None), None);
    }
    #[test]
    fn scan_outcomes_preserve_coverage_instead_of_claiming_no_users() {
        let mut coverage = Coverage::default();
        assert_eq!(coverage.outcome(0, 0), Outcome::Observed);
        coverage.denied = 1;
        assert_eq!(coverage.outcome(0, 0), Outcome::PermissionDenied);
        coverage.truncated = true;
        assert_eq!(coverage.outcome(0, 0), Outcome::Truncated);
        coverage.timed_out = true;
        assert_eq!(coverage.outcome(0, 0), Outcome::TimedOut);
    }
    #[test]
    fn terminal_numbers_decode_high_minor_bits_without_losing_sign_bits() {
        let mut stat = stat::Stat {
            pid: 42,
            start: 100,
            name: "fixture".into(),
            parent: 1,
            group: 42,
            session: 42,
            tty: 0,
            foreground: 42,
        };
        let minor = 0xabcde;
        stat.tty = (136 << 8) | (minor & 255) | ((minor & !255) << 12);
        assert!(tty_matches(
            &Device {
                character: true,
                identity: Vec::new(),
                rdev: libc::makedev(136, minor)
            },
            &stat
        ));
    }
}
