//! Quota-enforced RAM views backed by fixed-size images on verified tmpfs.
use crate::{Result, mounts, root::Root, *};
use little_clip_core::{kernel_interface, process::CommandRequest};
use std::{
    fs::{self, File},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    process::{Child, Stdio},
    time::{Duration, Instant},
};

const MIB: u64 = 1024 * 1024;
const PREFIX: &str = "little-clip-ram";
const HELPER_MEMORY: u64 = 512 * MIB;
pub(crate) struct Budget {
    pub bytes: u64,
    pub inodes: u64,
}

pub(crate) fn directory(config: &DriveConfig) -> Result<Root> {
    let mut candidates = vec![config.ram_directory.clone()];
    if config.ram_directory == Path::new("/tmp") {
        if let Some(path) = std::env::var_os("XDG_RUNTIME_DIR") {
            candidates.push(path.into());
        }
        candidates.push("/dev/shm".into());
    }
    for path in candidates {
        if let Ok(root) = Root::open(&path)
            && let Ok((pin, _)) = root.writable_directory()
            && rustix::fs::fstatfs(&pin).is_ok_and(|stats| stats.f_type as u32 == 0x01021994)
        {
            return Ok(root);
        }
    }
    Err("No writable/searchable verified tmpfs RAM destination; configure ram_directory or mount a writable tmpfs".into())
}

pub(crate) fn helper() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("LITTLE_CLIP_RAM_FS_HELPER") {
        let path = PathBuf::from(path);
        if !path.is_absolute() || !path.is_file() {
            return Err(
                "RAM filesystem helper override must name an existing absolute file".into(),
            );
        }
        return Ok(path);
    }
    let exe = std::env::current_exe()?;
    let parent = exe.parent().ok_or("Missing executable directory")?;
    let profile = if matches!(
        parent.file_name().and_then(|v| v.to_str()),
        Some("deps" | "examples")
    ) {
        parent.parent().ok_or("Missing Cargo profile directory")?
    } else {
        parent
    };
    let path = profile.join("drive-tools/fuse2fs");
    if !path.is_file() {
        return Err("RAM filesystem helper is missing; run scripts/build_drive_tools.py for this build profile".into());
    }
    Ok(path)
}

pub(crate) fn budget(
    config: &DriveConfig,
    capacity: u64,
    files: u64,
    bytes: u64,
) -> Result<Budget> {
    let inodes = files
        .saturating_add(files / 2)
        .saturating_add(4096)
        .min(config.ram_session_max_entries);
    let overhead = inodes.saturating_mul(512).saturating_add(64 * MIB);
    let bytes = capacity
        .saturating_add(capacity / 4)
        .max(
            bytes
                .saturating_add(bytes / 4)
                .saturating_add(files.saturating_mul(4096)),
        )
        .saturating_add(overhead)
        .max(128 * MIB)
        .div_ceil(MIB)
        .saturating_mul(MIB);
    if bytes > config.ram_session_max_bytes || inodes <= files.saturating_add(128) {
        return Err("Expanded files exceed the configured RAM session byte or inode limit".into());
    }
    Ok(Budget { bytes, inodes })
}

pub(crate) fn validate_quotas(config: &DriveConfig, budgets: &[Budget]) -> Result<()> {
    let capacity = budgets
        .iter()
        .fold(0_u64, |sum, b| sum.saturating_add(b.bytes));
    let inodes = budgets
        .iter()
        .fold(0_u64, |sum, b| sum.saturating_add(b.inodes));
    if budgets.len() > config.ram_max_sessions
        || capacity > config.ram_total_max_bytes
        || inodes > config.ram_total_max_entries
        || budgets.iter().any(|b| {
            b.bytes > config.ram_session_max_bytes || b.inodes > config.ram_session_max_entries
        })
    {
        return Err("Aggregate or per-session RAM reservation limit exceeded; save/discard sessions before reducing limits".into());
    }
    Ok(())
}

pub(crate) fn admit(
    config: &DriveConfig,
    budget: &Budget,
    sessions: &[RamSession],
    stats: &rustix::fs::StatVfs,
    available: u64,
) -> Result<()> {
    if sessions.len() >= config.ram_max_sessions || sessions.iter().any(|s| s.backing.is_none()) {
        return Err(
            "RAM session limit reached; save/discard legacy or unused sessions first".into(),
        );
    }
    let mut budgets = sessions
        .iter()
        .filter_map(|s| s.backing.as_ref())
        .map(|b| Budget {
            bytes: b.capacity,
            inodes: b.inodes,
        })
        .collect::<Vec<_>>();
    budgets.push(Budget {
        bytes: budget.bytes,
        inodes: budget.inodes,
    });
    validate_quotas(config, &budgets)?;
    let mut reserved = budget.bytes;
    for backing in sessions.iter().filter_map(|s| s.backing.as_ref()) {
        let (_, image) = open(backing)?;
        reserved = reserved.saturating_add(
            backing
                .capacity
                .saturating_sub(image.metadata()?.blocks().saturating_mul(512)),
        );
    }
    let helper_memory = HELPER_MEMORY.saturating_mul(sessions.len().saturating_add(1) as u64);
    if available < config.ram_min_available
        || available
            < reserved
                .saturating_add(helper_memory)
                .saturating_add(1024 * MIB)
        || stats.f_bavail.saturating_mul(stats.f_frsize) < reserved.saturating_add(64 * MIB)
        || stats.f_favail < 1024
    {
        return Err(
            "Insufficient RAM/tmpfs headroom after reserving existing sessions' growth".into(),
        );
    }
    Ok(())
}

pub(crate) fn prepare(
    base: &Root,
    budget: &Budget,
    record: impl FnOnce(&RamBacking) -> Result<()>,
) -> Result<RamBacking> {
    base.validate()?;
    let base_pin = base.directory(Path::new(""), false)?;
    let directory = tempfile::Builder::new()
        .prefix("little-clip-ram-")
        .tempdir_in(crate::root::anchor(&base_pin))?;
    let root = Root::open(directory.path())?;
    let directory_pin = root.directory(Path::new(""), false)?;
    File::open(crate::root::anchor(&directory_pin))?
        .set_permissions(fs::Permissions::from_mode(0o700))?;
    root.ensure_directory(Path::new("contents"))?;
    let parent = root.directory(Path::new(""), false)?;
    let image = File::from(rustix::fs::openat(
        &parent,
        "volume.ext4",
        rustix::fs::OFlags::RDWR
            | rustix::fs::OFlags::CLOEXEC
            | rustix::fs::OFlags::CREATE
            | rustix::fs::OFlags::EXCL
            | rustix::fs::OFlags::NOFOLLOW,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )?);
    image.set_len(budget.bytes)?;
    let backing = RamBacking {
        directory: root.path.clone(),
        directory_identity: root.storage_identity()?,
        image_identity: StorageIdentity::pinned(&image)?,
        capacity: budget.bytes,
        inodes: budget.inodes,
        mounted: false,
        initialized: false,
    };
    record(&backing)?;
    // Leave room for mke2fs's per-block-group inode rounding, then verify actual limits on mount.
    let padding = budget
        .bytes
        .div_ceil(128 * MIB)
        .saturating_mul(16)
        .saturating_add(32);
    let requested = budget.inodes.saturating_sub(padding);
    let (request, fd) = CommandRequest::new("mke2fs").inherit_handle(&image)?;
    let output = request
        .args([
            "-q",
            "-F",
            "-t",
            "ext4",
            "-b",
            "4096",
            "-I",
            "256",
            "-g",
            "32768",
            "-m",
            "1",
            "-O",
            "^has_journal",
            "-N",
        ])
        .arg(requested.to_string())
        .arg("-E")
        .arg(format!(
            "root_owner={}:{},lazy_itable_init=0",
            rustix::process::getuid().as_raw(),
            rustix::process::getgid().as_raw()
        ))
        .arg(format!("/proc/self/fd/{fd}"))
        .resource_limit(rustix::process::Resource::As, HELPER_MEMORY)
        .resource_limit(rustix::process::Resource::Fsize, budget.bytes)
        .timeout(Duration::from_secs(120))
        .max_output_bytes(8192)
        .own_process_group()
        .die_with_parent()
        .run()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Could not format bounded RAM image: {}",
            output.stderr_lossy()
        )
        .into());
    }
    // mke2fs creates a root-owned lost+found even with root_owner. Remove it before
    // publication so it never participates in a union back onto embedded media.
    let (request, fd) = CommandRequest::new("debugfs").inherit_handle(&image)?;
    let output = request
        .args(["-w", "-R", "rmdir /lost+found"])
        .arg(format!("/proc/self/fd/{fd}"))
        .resource_limit(rustix::process::Resource::As, HELPER_MEMORY)
        .resource_limit(rustix::process::Resource::Fsize, budget.bytes)
        .timeout(Duration::from_secs(30))
        .max_output_bytes(8192)
        .own_process_group()
        .die_with_parent()
        .run()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "RAM filesystem initialization failed: {}",
            output.stderr_lossy()
        )
        .into());
    }
    image.sync_all()?;
    root.validate()?;
    let _ = directory.keep();
    Ok(backing)
}

pub(crate) fn open(backing: &RamBacking) -> Result<(Root, File)> {
    let root = Root::open(&backing.directory)?;
    if root.path != backing.directory
        || root.storage_identity()? != backing.directory_identity
        || !root
            .path
            .file_name()
            .is_some_and(|n| n.as_encoded_bytes().starts_with(b"little-clip-ram-"))
    {
        return Err("RAM backing directory identity changed; data retained".into());
    }
    let parent = root.directory(Path::new(""), false)?;
    if rustix::fs::fstatfs(&parent)?.f_type as u32 != 0x01021994 {
        return Err("RAM backing is no longer on tmpfs".into());
    }
    let pinned = kernel_interface::pin(&crate::root::anchor(&parent).join("volume.ext4"), false)?;
    if !pinned.metadata()?.is_file() || kernel_interface::classify_handle(&pinned)?.protected() {
        return Err("RAM backing is not an ordinary file; refusing I/O".into());
    }
    let image = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(crate::root::anchor(&pinned))?;
    if !image.metadata()?.is_file()
        || StorageIdentity::pinned(&image)? != backing.image_identity
        || image.metadata()?.len() != backing.capacity
    {
        return Err("RAM image identity or capacity changed; refusing access".into());
    }
    Ok((root, image))
}

pub(crate) struct Mounted {
    child: Child,
    path: PathBuf,
    id: String,
    mount_id: Option<u64>,
}
impl Mounted {
    pub fn start(
        backing: &RamBacking,
        id: &str,
        fresh: bool,
        previous_mount: Option<u64>,
    ) -> Result<Self> {
        Self::start_with(
            backing,
            id,
            fresh,
            previous_mount,
            CommandRequest::new(helper()?).args(["-f", "-s"]),
            |_| {},
        )
    }
    #[cfg(test)]
    pub fn start_gated(backing: &RamBacking, id: &str, control: &File) -> Result<Self> {
        let helper = std::env::var_os("LITTLE_CLIP_TEST_RAM_GATE_HELPER").ok_or(
            "Build the separate --test-io-gate helper and set LITTLE_CLIP_TEST_RAM_GATE_HELPER",
        )?;
        let (request, fd) = CommandRequest::new(helper).inherit_handle(control)?;
        // Keep metadata responsive while one test-controlled data write waits.
        Self::start_with(backing, id, true, None, request.arg("-f"), move |command| {
            command.env("LITTLE_CLIP_FUSE_TEST_GATE_FD", fd.to_string());
        })
    }
    fn start_with(
        backing: &RamBacking,
        id: &str,
        fresh: bool,
        previous_mount: Option<u64>,
        request: CommandRequest,
        configure: impl FnOnce(&mut std::process::Command),
    ) -> Result<Self> {
        let (root, image) = open(backing)?;
        // The inherited descriptor retains this lock for the native server's whole lifetime.
        // SAFETY: flock has no pointer arguments and image is a live owned descriptor.
        if unsafe { libc::flock(image.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("RAM image is still owned by another helper; retry after it exits".into());
        }
        let path = root.path.join("contents");
        unmount(&path, id, previous_mount)?;
        if !fresh {
            let (request, fd) = CommandRequest::new("e2fsck").inherit_handle(&image)?;
            let output = request
                .arg("-p")
                .arg(format!("/proc/self/fd/{fd}"))
                .resource_limit(rustix::process::Resource::As, HELPER_MEMORY)
                .resource_limit(rustix::process::Resource::Fsize, backing.capacity)
                .timeout(Duration::from_secs(120))
                .max_output_bytes(8192)
                .own_process_group()
                .die_with_parent()
                .run()
                .map_err(|e| e.to_string())?;
            if !matches!(output.status.code(), Some(0 | 1)) {
                return Err(format!(
                    "RAM image needs recovery; backing retained: {}{}",
                    output.stdout_lossy(),
                    output.stderr_lossy()
                )
                .into());
            }
        }
        let (request, fd) = request.inherit_handle(&image)?;
        let mut command = request.arg("-o")
            .arg(format!("no_default_opts,subtype=clip-ram,fsname={PREFIX}:{id},nodev,nosuid,cache_size=4m,attr_timeout=0,entry_timeout=0,negative_timeout=0"))
            .arg(format!("/proc/self/fd/{fd}")).arg(&path)
            .resource_limit(rustix::process::Resource::As, HELPER_MEMORY)
            .resource_limit(rustix::process::Resource::Fsize, backing.capacity)
            .own_process_group().die_with_parent().build_command();
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure(&mut command);
        let mut mounted = Self {
            child: command.spawn()?,
            path,
            id: id.into(),
            mount_id: None,
        };
        mounted.await_ready(backing, fresh)?;
        Ok(mounted)
    }
    fn await_ready(&mut self, backing: &RamBacking, fresh: bool) -> Result<()> {
        let started = Instant::now();
        loop {
            if let Some(observed) = mounts::owned_mount(&self.path, PREFIX, &self.id)? {
                self.mount_id = Some(observed.id);
                let file = kernel_interface::pin_content_directory(&self.path)?;
                let stats = rustix::fs::fstatvfs(&file)?;
                if stats.f_files > backing.inodes
                    || stats.f_blocks.saturating_mul(stats.f_frsize) > backing.capacity
                {
                    return Err("Formatted RAM filesystem exceeds its reserved quota".into());
                }
                if fresh {
                    match fs::remove_dir(self.path.join("lost+found")) {
                        Ok(()) => (),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                        Err(e) => return Err(e.into()),
                    }
                }
                return Ok(());
            }
            if let Some(status) = self.child.try_wait()? {
                return Err(format!("RAM filesystem helper exited: {status}").into());
            }
            if started.elapsed() > Duration::from_secs(5) {
                return Err("RAM filesystem helper did not publish a ready mount".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    pub fn exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }
    #[cfg(test)]
    pub fn with_suspended(&mut self, test: impl FnOnce()) {
        struct Resume<'a>(&'a mut Child);
        impl Drop for Resume<'_> {
            fn drop(&mut self) {
                // SAFETY: the borrowed child remains unreaped for this scope.
                unsafe { libc::kill(self.0.id() as i32, libc::SIGCONT) };
            }
        }
        assert!(!self.exited(), "Cannot suspend an exited fixture helper");
        let resume = Resume(&mut self.child);
        let pid = resume.0.id() as i32;
        // SAFETY: this is our live, unreaped fixture child. No other owner waits.
        assert_eq!(unsafe { libc::kill(pid, libc::SIGSTOP) }, 0);
        let mut status = 0;
        // SAFETY: status is writable; WUNTRACED observes the owned child's stop.
        assert_eq!(
            unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) },
            pid
        );
        assert!(libc::WIFSTOPPED(status));
        assert_eq!(libc::WSTOPSIG(status), libc::SIGSTOP);
        test();
        drop(resume);
    }
}
impl Drop for Mounted {
    fn drop(&mut self) {
        if !self.exited() {
            // SIGTERM lets libfuse stop dispatch and op_destroy flush metadata before exit.
            // SAFETY: this is our unreaped child; its PID cannot have been reused.
            unsafe {
                libc::kill(self.child.id() as i32, libc::SIGTERM);
            }
            let started = Instant::now();
            while !self.exited() && started.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(20));
            }
            if !self.exited() {
                let _ = self.child.kill();
            }
        }
        let _ = self.child.wait();
        let _ = unmount(&self.path, &self.id, self.mount_id);
    }
}
fn unmount(path: &Path, id: &str, expected: Option<u64>) -> Result<()> {
    if let Some(observed) = mounts::owned_mount(path, PREFIX, id)? {
        if expected != Some(observed.id) {
            return Err("RAM mount identity changed; refusing unmount".into());
        }
        let output = CommandRequest::new("fusermount3")
            .arg("-uz")
            .arg(path)
            .timeout(Duration::from_secs(10))
            .max_output_bytes(4096)
            .run()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            return Err(format!("RAM unmount failed: {}", output.stderr_lossy()).into());
        }
    } else if mounts::inventory()?.iter().any(|m| m.target == path) {
        return Err("RAM mountpoint contains another filesystem; refusing to unmount it".into());
    }
    Ok(())
}

pub(crate) fn check_discard(backing: &RamBacking) -> Result<()> {
    let (root, _) = open(backing)?;
    let pin = root.directory(Path::new(""), false)?;
    for entry in fs::read_dir(crate::root::anchor(&pin))? {
        let name = entry?.file_name();
        if name != "contents" && name != "volume.ext4" {
            return Err(
                "RAM backing directory has unexpected files; move them aside before discarding"
                    .into(),
            );
        }
    }
    root.validate()?;
    Ok(())
}

pub(crate) fn discard(backing: &RamBacking, id: &str) -> Result<()> {
    check_discard(backing)?;
    let (root, image) = open(backing)?;
    // SAFETY: this live descriptor proves no other inherited image owner remains.
    if unsafe { libc::flock(image.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("RAM image helper has not exited; refusing discard".into());
    }
    unmount(&root.path.join("contents"), id, None)?;
    root.validate()?;
    let pin = root.directory(Path::new(""), false)?;
    fs::remove_dir(crate::root::anchor(&pin).join("contents"))?;
    fs::remove_file(crate::root::anchor(&pin).join("volume.ext4"))?;
    root.validate()?;
    fs::remove_dir(&root.path)?;
    Ok(())
}

#[cfg(test)]
mod tests;
