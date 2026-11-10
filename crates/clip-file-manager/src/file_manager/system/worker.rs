use super::{Document, Fact, Relation};
use little_clip_core::{
    BackgroundJobs,
    kernel_interface::{self, Kind},
};
use std::{
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::{fs::MetadataExt, process::CommandExt},
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct ManagedChild {
    id: uuid::Uuid,
    child: Child,
    retired: bool,
    response: Vec<u8>,
}

struct Checkpoint {
    path: PathBuf,
    source: Option<crate::file_source::FileSource>,
    failed: bool,
}
impl Checkpoint {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            source: None,
            failed: false,
        }
    }
    fn store(&mut self, document: &Document) {
        if self.failed {
            return;
        }
        match self.write(document) {
            Ok(source) => self.source = Some(source),
            Err(_) => self.failed = true,
        }
    }
    fn write(&self, document: &Document) -> Result<crate::file_source::FileSource, String> {
        let bytes = serde_json::to_vec(document).map_err(|error| error.to_string())?;
        if bytes.len() > 1024 * 1024 {
            return Err("Partial observation exceeds output budget".into());
        }
        if let Some(source) = &self.source {
            return source.write(&self.path, &bytes);
        }
        let temporary = tempfile::NamedTempFile::new_in(
            self.path
                .parent()
                .ok_or("Missing partial output directory")?,
        )
        .map_err(|error| error.to_string())?;
        crate::file_source::FileSource::capture(temporary.path())?.write(&self.path, &bytes)
    }
}

static CHILDREN: LazyLock<Mutex<Vec<ManagedChild>>> = LazyLock::new(|| Mutex::new(Vec::new()));
static OPTIONAL_CHILDREN: LazyLock<Mutex<Vec<ManagedChild>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));
#[derive(Clone, Copy)]
pub(super) enum Budget {
    Passive,
    Optional,
}
impl Budget {
    fn children(self) -> &'static Mutex<Vec<ManagedChild>> {
        match self {
            Self::Passive => &CHILDREN,
            Self::Optional => &OPTIONAL_CHILDREN,
        }
    }
    fn limit(self) -> usize {
        match self {
            Self::Passive => 2,
            Self::Optional => 1,
        }
    }
}
static CLOSING: AtomicBool = AtomicBool::new(false);
static JOBS: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(2, 4));
pub(in crate::file_manager) fn jobs() -> &'static BackgroundJobs {
    &JOBS
}
fn reap(children: &mut Vec<ManagedChild>) {
    children.retain_mut(|entry| !entry.retired || !matches!(entry.child.try_wait(), Ok(Some(_))));
}
pub(super) fn collect(
    path: &Path,
    token: &AtomicBool,
    previous: Option<&Document>,
) -> Result<Document, String> {
    collect_with(path, token, |output| {
        if let Some(previous) = previous {
            write_output(&output.with_extension("seed.json"), previous)?;
        }
        spawn(path, output)
    })
}
fn collect_with(
    path: &Path,
    token: &AtomicBool,
    start: impl FnOnce(&Path) -> Result<uuid::Uuid, String>,
) -> Result<Document, String> {
    if token.load(Ordering::Relaxed) {
        return Err("System observation cancelled".into());
    }
    let event_revision = super::events::revision();
    let dir = tempfile::Builder::new()
        .prefix("veylar-system-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    let output = dir.path().join("result.json");
    let id = start(&output)?;
    let completion = wait(id, token);
    if token.load(Ordering::Relaxed) {
        return Err("System observation cancelled".into());
    }
    let result = if completion.is_ok() {
        output
    } else {
        output.with_extension("partial.json")
    };
    let bytes = crate::file_source::read_file_bytes(&result, 1024 * 1024, true, false)
        .map_err(|e| e.to_string())?;
    let mut document: Document = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let current = interface(path)?;
    if !super::identity::same_interface(&document, &current)
        || !super::identity::same_observation(&document, &current)
    {
        return Err("System object changed before result acceptance".into());
    }
    if let Err(error) = completion {
        document.last_error = Some(error.clone());
        let kind = if error.contains("timed out") {
            std::io::ErrorKind::TimedOut
        } else {
            std::io::ErrorKind::Other
        };
        document.facts.push(Fact::failure(
            "Collection completion",
            document.path.clone(),
            std::io::Error::new(kind, error),
        ));
    }
    if super::events::affected(event_revision, document.event_paths()) {
        return Err("System object changed during observation; retry".into());
    }
    document.event_revision = event_revision;
    super::identity::stamp(&mut document);
    Ok(document)
}
fn spawn(path: &Path, output: &Path) -> Result<uuid::Uuid, String> {
    // Execute the running inode, including after a rebuild unlinks its old pathname.
    // This also keeps helper response types matched to their owning application.
    let mut command = Command::new("/proc/self/exe");
    command
        .args([
            std::ffi::OsStr::new("--internal-system-inspector"),
            path.as_os_str(),
            output.as_os_str(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    spawn_command(command)
}
pub(super) fn spawn_command(command: Command) -> Result<uuid::Uuid, String> {
    spawn_in(Budget::Passive, command)
}
pub(super) fn spawn_in(budget: Budget, mut command: Command) -> Result<uuid::Uuid, String> {
    let id = uuid::Uuid::new_v4();
    let mut children = budget.children().lock().unwrap_or_else(|e| e.into_inner());
    if CLOSING.load(Ordering::Acquire) {
        return Err("System collector admission is closed".into());
    }
    reap(&mut children);
    if children.len() >= budget.limit() {
        return Err("System collectors are busy or awaiting kernel I/O completion".into());
    }
    // SAFETY: pre_exec only makes async-signal-safe Linux syscalls.
    unsafe {
        command.pre_exec(|| {
            let parent = libc::getppid();
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != parent || parent == 1 {
                libc::_exit(2);
            }
            Ok(())
        });
    }
    children.push(ManagedChild {
        id,
        child: command.spawn().map_err(|e| e.to_string())?,
        retired: false,
        response: Vec::new(),
    });
    if let Some(stdout) = children
        .last()
        .and_then(|entry| entry.child.stdout.as_ref())
    {
        // SAFETY: stdout is a live owned pipe descriptor; only its status flags change.
        let flags = unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                < 0
        {
            let entry = children.last_mut().unwrap();
            entry.retired = true;
            let _ = entry.child.kill();
            return Err("Cannot configure bounded helper response pipe".into());
        }
    }
    Ok(id)
}
pub(super) fn wait(id: uuid::Uuid, token: &AtomicBool) -> Result<(), String> {
    wait_in(Budget::Passive, id, token).map(|_| ())
}
pub(super) fn wait_in(
    budget: Budget,
    id: uuid::Uuid,
    token: &AtomicBool,
) -> Result<Vec<u8>, String> {
    let started = Instant::now();
    loop {
        let mut children = budget.children().lock().unwrap_or_else(|e| e.into_inner());
        let Some(index) = children.iter().position(|entry| entry.id == id) else {
            return Err("System collector ownership lost".into());
        };
        let entry = &mut children[index];
        // Reserve response time for parsing partial output and identity revalidation.
        let helper_limit = super::RESPONSE_DEADLINE.saturating_sub(Duration::from_millis(250));
        if token.load(Ordering::Relaxed) || started.elapsed() > helper_limit {
            entry.retired = true;
            let _ = entry.child.kill();
            reap(&mut children);
            return Err("System observation cancelled or timed out".into());
        }
        if let Err(error) = drain_response(entry) {
            entry.retired = true;
            let _ = entry.child.kill();
            return Err(error);
        }
        match entry.child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => {
                // A final bounded drain handles bytes written immediately before exit.
                let drained = drain_response(entry);
                let response = children.swap_remove(index).response;
                drained?;
                if !status.success() {
                    return Err(
                        "System metadata is unavailable or changed during inspection".into(),
                    );
                }
                drop(children);
                return Ok(response);
            }
            None => {
                drop(children);
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

/// Nonblocking stdout collection, bounded in both memory and work per wait poll.
fn drain_response(entry: &mut ManagedChild) -> Result<(), String> {
    let Some(stdout) = entry.child.stdout.as_mut() else {
        return Ok(());
    };
    let mut buffer = [0_u8; 4096];
    for _ in 0..64 {
        match stdout.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(count) => {
                if entry.response.len() + count > 256 * 1024 {
                    return Err("Helper response exceeds 256 KiB budget".into());
                }
                entry.response.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

pub(super) fn run(path: &Path, output: &Path) -> Result<(), String> {
    kernel_interface::require_content(
        output
            .parent()
            .ok_or("Missing observation output directory")?,
    )
    .map_err(|e| e.to_string())?;
    let mut checkpoint = Checkpoint::new(output.with_extension("partial.json"));
    let seed = match crate::file_source::read_file_bytes(
        &output.with_extension("seed.json"),
        1024 * 1024,
        true,
        false,
    ) {
        Ok(bytes) => Some(serde_json::from_slice::<Document>(&bytes).map_err(|e| e.to_string())?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.to_string()),
    };
    let document = metadata(
        path,
        &mut |document| checkpoint.store(document),
        seed.as_ref(),
    )?;
    write_output(output, &document)
}
pub(super) fn write_output(output: &Path, value: &impl serde::Serialize) -> Result<(), String> {
    kernel_interface::require_content(output.parent().ok_or("Missing output directory")?)
        .map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    if bytes.len() > 1024 * 1024 {
        return Err("System observation exceeds output budget".into());
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())
}

pub(super) fn interface(path: &Path) -> Result<Document, String> {
    let pinned = kernel_interface::pin(path, true).map_err(|e| e.to_string())?;
    let kind = kernel_interface::classify_handle(&pinned).map_err(|e| e.to_string())?;
    let stat = pinned.metadata().map_err(|e| e.to_string())?;
    let context = super::identity::ObjectContext::pinned(&pinned);
    if !kind.semantic() && !(kind == Kind::Content && stat.is_file()) {
        return Err("Not a kernel interface or regular backing file".into());
    }
    let descriptor_name = std::fs::read_link(format!("/proc/self/fd/{}", pinned.as_raw_fd()))
        .map_err(|e| e.to_string())?;
    let namespace_handle = matches!(
        kind,
        Kind::Kernel {
            filesystem: kernel_interface::Filesystem::Namespace
        }
    );
    // nsfs descriptor labels (e.g. mnt:[inode]) are not navigable pathnames.
    let canonical = if namespace_handle {
        std::path::absolute(path).map_err(|e| e.to_string())?
    } else {
        descriptor_name.clone()
    };
    let title = match kind {
        Kind::Character { major, minor } => format!("Character device {major}:{minor}"),
        Kind::Block { major, minor } => format!("Block device {major}:{minor}"),
        Kind::Kernel { filesystem } => format!(
            "{filesystem:?} {}",
            if stat.is_dir() { "object" } else { "attribute" }
        ),
        Kind::Content => "File loop associations".into(),
        _ => unreachable!(),
    };
    let mut document = Document {
        path: canonical.clone(),
        interface_directory: stat.is_dir(),
        kind,
        title,
        namespace: super::identity::Namespace::current(),
        kernel_identity: None,
        interface_context: context.as_ref().ok().copied(),
        kernel_context: None,
        generation: 0,
        event_revision: 0,
        last_error: None,
        identity: super::identity::interface_fingerprint(kind, &stat),
        observed_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
        facts: Vec::new(),
        relations: Vec::new(),
        storage: None,
        topology: None,
        cpu: None,
        telemetry: None,
        numa: None,
    };
    describe_interface(&mut document, &stat, context.err());
    if namespace_handle {
        describe_namespace(&mut document, &descriptor_name, stat.ino());
        return Ok(document);
    }
    if kind == Kind::Content {
        return Ok(document);
    }
    discover_interface_relationships(&mut document);
    Ok(document)
}

fn describe_interface(
    document: &mut Document,
    stat: &std::fs::Metadata,
    context_error: Option<std::io::Error>,
) {
    for (label, value) in [
        ("Permissions", format!("{:04o}", stat.mode() & 0o7777)),
        ("Owner", format!("{}:{}", stat.uid(), stat.gid())),
        ("Access", "Unverified; no device opened".into()),
    ] {
        let mut fact = Fact::text(label, value, document.path.clone());
        fact.interface = true;
        document.facts.push(fact);
    }
    if let Some(error) = context_error {
        let mut fact = Fact::failure(
            "Mounted object identity (reuse disabled)",
            document.path.clone(),
            error,
        );
        fact.interface = true;
        document.facts.push(fact);
    }
}

/// Describes a pinned namespace object without interpreting its symbolic label as a path.
fn describe_namespace(document: &mut Document, descriptor_name: &Path, inode: u64) {
    document.title = "Namespace handle".into();
    for (label, value) in [
        (
            "Namespace descriptor",
            descriptor_name.to_string_lossy().into_owned(),
        ),
        ("Namespace inode", inode.to_string()),
        (
            "Inspection policy",
            "Metadata only; no namespace entry, payload reads or namespace ioctls".into(),
        ),
        (
            "Identity scope",
            "Inode identifies this observed namespace handle, not a permanent global identity"
                .into(),
        ),
    ] {
        let mut fact = Fact::text(label, value, document.path.clone());
        fact.interface = true;
        document.facts.push(fact);
    }
}

/// Resolves device/sysfs topology separately from interface metadata and nsfs labels.
fn discover_interface_relationships(document: &mut Document) {
    let canonical = &document.path;
    let sys = match document.kind {
        Kind::Character {
            major: major @ (202 | 203),
            minor,
        } => {
            document.title = if major == 202 {
                "CPU MSR access interface"
            } else {
                "CPU CPUID access interface"
            }
            .into();
            let mut fact = Fact::text(
                "CPU access policy",
                "Sysfs topology and available CPU-local identification; the low-level CPU device is not opened",
                canonical.clone(),
            );
            fact.interface = true;
            document.facts.insert(0, fact);
            PathBuf::from(format!("/sys/devices/system/cpu/cpu{minor}"))
        }
        Kind::Character { major, minor } => PathBuf::from(format!("/sys/dev/char/{major}:{minor}")),
        Kind::Block { major, minor } => PathBuf::from(format!("/sys/dev/block/{major}:{minor}")),
        _ => {
            if document.interface_directory {
                canonical.clone()
            } else {
                canonical.parent().unwrap_or(&canonical).into()
            }
        }
    };
    discover_relationships(document, &sys);
}
fn metadata(
    path: &Path,
    checkpoint: &mut dyn FnMut(&Document),
    previous: Option<&Document>,
) -> Result<Document, String> {
    let mut document = interface(path)?;
    let previous = previous.filter(|old| {
        old.namespace.known()
            && super::identity::Key::for_document(old)
                == super::identity::Key::for_document(&document)
            && super::identity::observation_identity(old)
                == super::identity::observation_identity(&document)
    });
    checkpoint(&document);
    if document.kind == Kind::Content {
        super::storage::loops::collect(&mut document, checkpoint);
    }
    let device_namespace = matches!(
        document.kind,
        Kind::Kernel {
            filesystem: kernel_interface::Filesystem::DeviceNodes
                | kernel_interface::Filesystem::Devpts
        }
    );
    if !device_namespace
        && let Some(sys) = document
            .relations
            .iter()
            .find(|r| r.label == "Kernel object")
            .map(|r| r.target.clone())
    {
        if matches!(
            document.kind,
            Kind::Kernel {
                filesystem: kernel_interface::Filesystem::Cgroup
            }
        ) {
            super::cgroups::collect(&mut document, &sys, checkpoint);
        } else if !super::kernel_state::collect(&mut document, &sys, checkpoint) {
            super::collector::attributes(&mut document, &sys, checkpoint, previous);
        }
    }
    super::console::describe(&mut document);
    super::pseudo::describe(&mut document);
    super::pseudo::collect(&mut document);
    super::vfio::unmapped(&mut document);
    super::clock_watchdog::unmapped(&mut document);
    super::processes::describe(&mut document);
    checkpoint(&document);
    if device_namespace {
        super::collector::device_directory(&mut document);
        super::device_directories::collect(&mut document);
    }
    super::identity::verify_source(&document)?;
    Ok(document)
}
fn discover_relationships(document: &mut Document, sys: &Path) {
    if let Ok(sys) = sys.canonicalize() {
        if let Ok(file) = kernel_interface::pin(&sys, true) {
            if let Ok(stat) = file.metadata() {
                document.kernel_identity = Some(super::identity::fingerprint(&stat));
                match super::identity::ObjectContext::pinned(&file) {
                    Ok(context) => document.kernel_context = Some(context),
                    Err(error) => {
                        let mut fact = Fact::failure(
                            "Kernel object mount identity (reuse disabled)",
                            sys.clone(),
                            error,
                        );
                        fact.interface = true;
                        document.facts.push(fact);
                    }
                }
            }
        }
        document
            .relations
            .push(Relation::new("Kernel object", sys.clone()));
        for name in ["driver", "subsystem", "device"] {
            if let Ok(target) = sys.join(name).canonicalize() {
                document.relations.push(Relation::new(name, target));
            }
        }
        if let Some(parent) = sys.parent().filter(|_| {
            !matches!(
                document.kind,
                Kind::Kernel {
                    filesystem: kernel_interface::Filesystem::Cgroup
                }
            )
        }) {
            document
                .relations
                .push(Relation::new("Parent", parent.into()));
        }
    }
}

pub(super) fn shutdown() {
    // Publish before taking either budget lock: in-flight admissions are then
    // retired under their lock, and later requests cannot start another helper.
    CLOSING.store(true, Ordering::Release);
    super::processes::shutdown();
    for budget in [Budget::Passive, Budget::Optional] {
        let mut children = budget.children().lock().unwrap_or_else(|e| e.into_inner());
        for entry in &mut *children {
            entry.retired = true;
            let _ = entry.child.kill();
        }
        reap(&mut children);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_namespace_directories_do_not_inherit_sysfs_schemas_from_their_names() {
        for root in ["/dev/pts", "/dev/cpu"] {
            let path = Path::new(root);
            if !path.is_dir() {
                continue;
            }
            let document = metadata(path, &mut |_| {}, None).unwrap();
            assert!(document.cpu.is_none(), "{root} inherited a sysfs CPU model");
            assert!(
                document
                    .facts
                    .iter()
                    .all(|fact| !(fact.origin == "documented sysfs attribute"
                        && fact.source.starts_with(path)))
            );
        }
    }

    #[test]
    fn namespace_handle_preserves_navigable_alias_without_false_parent_relationships() {
        let temp = tempfile::tempdir().unwrap();
        let alias = temp.path().join("namespace.png");
        std::os::unix::fs::symlink("/proc/self/ns/mnt", &alias).unwrap();
        let document = metadata(&alias, &mut |_| {}, None).unwrap();
        assert_eq!(document.path, alias);
        assert_eq!(document.title, "Namespace handle");
        assert!(document.relations.is_empty());
        assert!(document.kernel_identity.is_none());
        assert!(
            document
                .facts
                .iter()
                .any(|f| f.label == "Namespace descriptor")
        );
        assert_eq!(
            document.identity,
            super::super::identity::fingerprint(&std::fs::metadata(&alias).unwrap())[..3]
        );
    }

    #[test]
    fn namespace_wrapper_timestamps_do_not_change_observation_identity() {
        let path = Path::new("/proc/self/ns/net");
        let first = interface(path).unwrap();
        std::thread::sleep(Duration::from_millis(10));
        let second = interface(path).unwrap();
        assert_eq!(first.identity.len(), 3);
        assert_eq!(
            super::super::identity::observation_identity(&first),
            super::super::identity::observation_identity(&second)
        );
    }

    #[test]
    fn cgroup_parents_are_owned_by_semantic_collection_not_generic_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("group");
        std::fs::create_dir(&root).unwrap();
        let mut document = interface(Path::new("/proc/uptime")).unwrap();
        document.kind = Kind::Kernel {
            filesystem: kernel_interface::Filesystem::Cgroup,
        };
        document.relations.clear();
        discover_relationships(&mut document, &root);
        assert!(
            !document
                .relations
                .iter()
                .any(|relation| relation.label == "Parent")
        );
        document.kind = Kind::Kernel {
            filesystem: kernel_interface::Filesystem::Sysfs,
        };
        document.relations.clear();
        discover_relationships(&mut document, &root);
        assert!(
            document
                .relations
                .iter()
                .any(|relation| relation.label == "Parent")
        );
    }

    struct OwnedChildren(Budget, Vec<uuid::Uuid>);
    impl Drop for OwnedChildren {
        fn drop(&mut self) {
            let mut children = self
                .0
                .children()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            for entry in children
                .iter_mut()
                .filter(|entry| self.1.contains(&entry.id))
            {
                entry.retired = true;
                let _ = entry.child.kill();
            }
            reap(&mut children);
        }
    }
    fn fixture(script: &str) -> Command {
        let mut command = Command::new("python3");
        command
            .args(["-c", script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    #[test]
    fn failed_helper_without_checkpoint_reports_unavailable_and_releases_its_exited_slot() {
        let mut owned = OwnedChildren(Budget::Passive, Vec::new());
        let started = Instant::now();
        let result = collect_with(Path::new("/proc/uptime"), &AtomicBool::new(false), |_| {
            let id = spawn_command(fixture("raise SystemExit(1)"))?;
            owned.1.push(id);
            Ok(id)
        });
        assert!(
            result.is_err(),
            "no checkpoint must not fabricate successful metadata"
        );
        assert!(started.elapsed() < super::super::RESPONSE_DEADLINE);
        let children = CHILDREN.lock().unwrap();
        assert!(
            owned
                .1
                .iter()
                .all(|id| children.iter().all(|entry| entry.id != *id))
        );
    }

    #[test]
    fn stalled_helper_returns_partial_facts_and_live_helper_admission_is_bounded() {
        let source = Path::new("/proc/uptime");
        let baseline = interface(source).unwrap();
        let mut owned = OwnedChildren(Budget::Passive, Vec::new());
        let started = Instant::now();
        let observed = collect_with(source, &AtomicBool::new(false), |output| {
            let mut command = fixture("import os, pathlib, signal; pathlib.Path(os.environ['RESULT']).write_text(os.environ['DOCUMENT']); signal.pause()");
            command.env("RESULT", output.with_extension("partial.json"))
                .env("DOCUMENT", serde_json::to_string(&baseline).unwrap());
            let id = spawn_command(command)?;
            owned.1.push(id);
            Ok(id)
        }).unwrap();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(observed.last_error.is_some());
        assert!(
            observed
                .facts
                .iter()
                .any(|fact| fact.label == "Permissions" && fact.value.is_some())
        );
        assert!(
            observed
                .facts
                .iter()
                .any(|fact| fact.label == "Collection completion"
                    && fact.outcome == super::super::model::Outcome::TimedOut)
        );
        assert_eq!(observed.identity.get(..3), baseline.identity.get(..3));

        for _ in 0..2 {
            let id = spawn_command(fixture("import signal; signal.pause()")).unwrap();
            owned.1.push(id);
        }
        let rejected = spawn_command(fixture("raise RuntimeError('must never spawn')"));
        assert!(rejected.unwrap_err().contains("busy"));
        assert!(CHILDREN.lock().unwrap().len() <= 2);
        for id in owned.1.iter().copied() {
            let _ = wait(id, &AtomicBool::new(true));
        }
    }

    #[test]
    fn shutdown_closes_admission_for_both_helper_budgets() {
        // Run irreversible global shutdown in a separate test executable.
        if std::env::var_os("VEYLAR_SYSTEM_SHUTDOWN_FIXTURE").is_none() {
            let mut command = Command::new("/proc/self/exe");
            command.args(["--exact", "file_manager::system::worker::tests::shutdown_closes_admission_for_both_helper_budgets", "--test-threads=1"])
                .env("VEYLAR_SYSTEM_SHUTDOWN_FIXTURE", "1")
                .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
            let id = spawn_in(Budget::Optional, command).unwrap();
            let _owned = OwnedChildren(Budget::Optional, vec![id]);
            let output = wait_in(Budget::Optional, id, &AtomicBool::new(false)).unwrap();
            assert!(String::from_utf8_lossy(&output).contains("1 passed"));
            return;
        }
        let mut passive = OwnedChildren(Budget::Passive, Vec::new());
        let mut optional = OwnedChildren(Budget::Optional, Vec::new());
        passive
            .1
            .push(spawn_in(Budget::Passive, fixture("import signal; signal.pause()")).unwrap());
        optional
            .1
            .push(spawn_in(Budget::Optional, fixture("import signal; signal.pause()")).unwrap());
        shutdown();
        for budget in [Budget::Passive, Budget::Optional] {
            assert!(
                spawn_in(budget, fixture("raise RuntimeError('must never start')"))
                    .unwrap_err()
                    .contains("closed")
            );
            let deadline = Instant::now() + Duration::from_secs(1);
            loop {
                let mut children = budget.children().lock().unwrap();
                assert!(children.iter().all(|entry| entry.retired));
                reap(&mut children);
                if children.is_empty() {
                    break;
                }
                drop(children);
                assert!(
                    Instant::now() < deadline,
                    "owned ordinary fixture did not exit after shutdown"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    #[test]
    fn optional_helper_pipe_is_bounded_and_retired_live_children_keep_their_slot() {
        let mut owned = OwnedChildren(Budget::Optional, Vec::new());
        let command = |script: &str| {
            let mut command = Command::new("python3");
            command
                .args(["-c", script])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            command
        };
        let id = spawn_in(
            Budget::Optional,
            command("import sys; sys.stdout.write('x'*131072)"),
        )
        .unwrap();
        owned.1.push(id);
        assert_eq!(
            wait_in(Budget::Optional, id, &AtomicBool::new(false))
                .unwrap()
                .len(),
            131072
        );
        let id = spawn_in(Budget::Optional, command("import time; time.sleep(20)")).unwrap();
        owned.1.push(id);
        {
            let mut children = OPTIONAL_CHILDREN.lock().unwrap();
            children
                .iter_mut()
                .find(|entry| entry.id == id)
                .unwrap()
                .retired = true;
        }
        assert!(spawn_in(Budget::Optional, command("pass")).is_err());
        assert_eq!(Budget::Passive.limit(), 2);
        assert_eq!(Budget::Optional.limit(), 1);
        assert!(!std::ptr::eq(
            Budget::Passive.children(),
            Budget::Optional.children()
        ));
        assert!(wait_in(Budget::Optional, id, &AtomicBool::new(true)).is_err());
    }

    #[test]
    fn partial_checkpoints_are_atomic_and_never_overwrite_an_unowned_destination() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("partial.json");
        let mut document = interface(Path::new("/proc/uptime")).unwrap();
        let mut checkpoint = Checkpoint::new(target.clone());
        checkpoint.store(&document);
        assert!(!checkpoint.failed);
        document.title = "Updated observation".into();
        checkpoint.store(&document);
        let stored: Document = serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        assert_eq!(stored.title, "Updated observation");
        std::fs::remove_file(&target).unwrap();
        std::fs::write(&target, b"replacement owned elsewhere").unwrap();
        checkpoint.store(&document);
        assert!(checkpoint.failed);
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"replacement owned elsewhere"
        );
    }
}
