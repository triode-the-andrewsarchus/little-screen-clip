use super::{Coverage, Device, Process, Snapshot, stat};
use little_clip_core::kernel_interface;
use std::{
    io,
    path::Path,
    time::{Duration, Instant},
};
const PID_LIMIT: usize = 1024;
const FD_LIMIT: usize = 32768;
const PER_PID: usize = 256;
const WALL_LIMIT: Duration = Duration::from_millis(500);
impl Coverage {
    fn error(&mut self, error: &io::Error) {
        match error.kind() {
            io::ErrorKind::PermissionDenied => self.denied += 1,
            io::ErrorKind::NotFound => self.vanished += 1,
            io::ErrorKind::InvalidData => self.malformed += 1,
            _ => self.other += 1,
        }
    }
}
pub(super) fn collect(root: &Path) -> io::Result<Snapshot> {
    let started = Instant::now();
    let mut snapshot = Snapshot::new();
    let entries = std::fs::read_dir(root)?;
    let mut retained_bytes = 0;
    for (index, entry) in entries.enumerate() {
        if index >= 4096 || snapshot.coverage.pids >= PID_LIMIT || snapshot.coverage.fds >= FD_LIMIT
        {
            snapshot.coverage.truncated = true;
            break;
        }
        if started.elapsed() >= WALL_LIMIT {
            snapshot.coverage.timed_out = true;
            break;
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                snapshot.coverage.error(&error);
                continue;
            }
        };
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        snapshot.coverage.pids += 1;
        match process(&entry.path(), pid, started, &mut snapshot.coverage) {
            Ok(process) => {
                let size = serde_json::to_vec(&process)
                    .map_err(io::Error::other)?
                    .len();
                retained_bytes += size;
                if retained_bytes > 768 * 1024 {
                    snapshot.coverage.truncated = true;
                    break;
                }
                snapshot.processes.push(process);
            }
            Err(error) => snapshot.coverage.error(&error),
        }
    }
    snapshot.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(snapshot)
}
fn process(
    path: &Path,
    pid: u32,
    started: Instant,
    coverage: &mut Coverage,
) -> io::Result<Process> {
    process_with(path, pid, started, coverage, stat::read)
}
fn process_with(
    path: &Path,
    pid: u32,
    started: Instant,
    coverage: &mut Coverage,
    read_stat: impl Fn(&Path) -> io::Result<stat::Stat>,
) -> io::Result<Process> {
    let before = read_stat(path)?;
    if before.pid != pid {
        return Err(io::Error::from(io::ErrorKind::NotFound));
    }
    let mut devices = Vec::new();
    match std::fs::read_dir(path.join("fd")) {
        Ok(entries) => descriptors(entries, started, coverage, &mut devices),
        Err(error) => coverage.error(&error),
    }
    let after = read_stat(path)?;
    if before.pid != after.pid || before.start != after.start {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "PID lifetime changed during scan",
        ));
    }
    Ok(Process {
        stat: after,
        devices,
    })
}
fn descriptors(
    entries: std::fs::ReadDir,
    started: Instant,
    coverage: &mut Coverage,
    devices: &mut Vec<Device>,
) {
    for (index, entry) in entries.enumerate() {
        if index >= PER_PID || coverage.fds >= FD_LIMIT {
            coverage.truncated = true;
            break;
        }
        if started.elapsed() >= WALL_LIMIT {
            coverage.timed_out = true;
            break;
        }
        coverage.fds += 1;
        let metadata = entry
            .and_then(|entry| kernel_interface::pin(&entry.path(), true))
            .and_then(|file| file.metadata());
        match metadata {
            Ok(metadata) => {
                if let Some(device) = Device::from_metadata(&metadata) {
                    if !devices.contains(&device) {
                        devices.push(device);
                    }
                }
            }
            Err(error) => coverage.error(&error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity(pid: u32, start: u64) -> stat::Stat {
        stat::Stat {
            pid,
            start,
            name: "fixture".into(),
            parent: 1,
            group: 42,
            session: 42,
            tty: 0,
            foreground: -1,
        }
    }
    fn descriptor_fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("42/fd");
        std::fs::create_dir_all(&path).unwrap();
        std::os::unix::fs::symlink("/dev/null", path.join("0")).unwrap();
        root
    }
    #[test]
    fn reused_pid_cannot_keep_the_previous_descriptors() {
        let root = descriptor_fixture();
        let path = root.path().join("42");
        let calls = std::cell::Cell::new(0);
        let mut coverage = Coverage::default();
        let result = process_with(&path, 42, Instant::now(), &mut coverage, |_| {
            let call = calls.get();
            calls.set(call + 1);
            Ok(identity(42, 100 + call))
        });
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
        assert_eq!(coverage.fds, 1);
    }
    #[test]
    fn descriptor_identity_is_metadata_only_and_vanishing_fds_remain_distinct() {
        let root = descriptor_fixture();
        let path = root.path().join("42");
        std::os::unix::fs::symlink("/dev/does-not-exist-fixture", path.join("fd/1")).unwrap();
        let mut coverage = Coverage::default();
        let process = process_with(&path, 42, Instant::now(), &mut coverage, |_| {
            Ok(identity(42, 100))
        })
        .unwrap();
        assert_eq!(process.devices.len(), 1);
        assert_eq!(coverage.vanished, 1);
        assert_eq!(libc::major(process.devices[0].rdev), 1);
        assert_eq!(libc::minor(process.devices[0].rdev), 3);
    }
    #[test]
    fn permission_failure_and_descriptor_bounds_never_become_successful_empty() {
        let mut coverage = Coverage::default();
        coverage.error(&io::Error::from(io::ErrorKind::PermissionDenied));
        assert_eq!(coverage.denied, 1);
        assert_eq!(coverage.vanished, 0);
        let root = tempfile::tempdir().unwrap();
        for index in 0..PER_PID + 1 {
            std::os::unix::fs::symlink("/dev/null", root.path().join(index.to_string())).unwrap();
        }
        let mut devices = Vec::new();
        descriptors(
            std::fs::read_dir(root.path()).unwrap(),
            Instant::now(),
            &mut coverage,
            &mut devices,
        );
        assert!(coverage.truncated);
        assert_eq!(coverage.fds, PER_PID);
    }
}
