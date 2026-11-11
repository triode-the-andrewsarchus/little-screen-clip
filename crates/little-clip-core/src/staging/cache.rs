//! Budgeted eviction for committed, regenerable cache entries.
use std::{ffi::OsStr, fs, io, os::unix::fs::MetadataExt, path::Path, sync::Mutex};

pub(crate) struct CacheBudget {
    bytes: u64,
    usage: Mutex<Usage>,
}
struct Usage {
    written: Option<u64>,
    over_budget: bool,
}
impl CacheBudget {
    pub(crate) const fn new(bytes: u64) -> Self {
        Self {
            bytes,
            usage: Mutex::new(Usage {
                written: None,
                over_budget: false,
            }),
        }
    }
    pub(crate) fn maintain(&self, directory: &Path, written: u64, eligible: fn(&OsStr) -> bool) {
        const SWEEP_AFTER_BYTES: u64 = 32 * 1024 * 1024;
        let mut usage = self.usage.lock().unwrap_or_else(|error| error.into_inner());
        let written = usage
            .written
            .map_or(SWEEP_AFTER_BYTES, |bytes| bytes.saturating_add(written));
        if usage.over_budget || written >= SWEEP_AFTER_BYTES {
            usage.over_budget = sweep(directory, self.bytes, eligible).unwrap_or(false);
            usage.written = Some(0);
        } else {
            usage.written = Some(written);
        }
    }
}

pub(crate) fn sweep(
    directory: &Path,
    budget: u64,
    eligible: fn(&OsStr) -> bool,
) -> io::Result<bool> {
    use std::os::fd::AsRawFd;
    let directory = crate::kernel_interface::pin_content_directory(directory)?;
    let anchor = std::path::PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let mut candidates = Vec::new();
    let mut total = 0u64;
    for entry in fs::read_dir(&anchor)?.flatten() {
        let name = entry.file_name();
        if !eligible(&name) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_file() {
            total = total.saturating_add(metadata.len());
            candidates.push((metadata.modified().ok(), name, metadata.len()));
        }
    }
    if total > budget {
        candidates.sort_unstable();
        for (_, name, bytes) in candidates {
            if total <= budget {
                break;
            }
            if remove_idle(&anchor.join(name)).is_ok() {
                total = total.saturating_sub(bytes);
            }
        }
    }
    Ok(total > budget)
}

fn remove_idle(path: &Path) -> io::Result<()> {
    let file = crate::file_source::open_regular_file(path, false)?;
    file.try_lock().map_err(io::Error::other)?;
    let held = file.metadata()?;
    let named = fs::symlink_metadata(path)?;
    if !named.is_file() || (held.dev(), held.ino()) != (named.dev(), named.ino()) {
        return Err(io::Error::other("Cache entry changed before eviction"));
    }
    fs::remove_file(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deferred_eviction_retries_after_readers_release_without_more_writes() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("cached");
        fs::write(&path, b"cached bytes").unwrap();
        let reader = fs::File::open(&path).unwrap();
        reader.try_lock_shared().unwrap();
        let budget = CacheBudget::new(0);
        budget.maintain(root.path(), 0, |_| true);
        assert!(path.exists());
        reader.unlock().unwrap();
        budget.maintain(root.path(), 0, |_| true);
        assert!(!path.exists());
    }
}
