//! Bounded local JSON state with pinned-directory atomic updates.
use little_clip_core::{
    BackgroundJobs,
    file_source::{self, FileSource},
    kernel_interface,
};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs::File,
    io::Read,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    sync::LazyLock,
};

// Slow local-state mounts retain this one shared worker through actual exit;
// accepted writes never consume or block browser/navigation admission.
pub(super) static IO: LazyLock<BackgroundJobs> = LazyLock::new(|| BackgroundJobs::new(1, 2));

pub(super) fn read<T: Default + DeserializeOwned>(
    path: &Path,
    limit: usize,
) -> Result<(T, Option<FileSource>), String> {
    let mut input = match file_source::open_regular_file(path, false) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((T::default(), None));
        }
        result => result.map_err(|e| e.to_string())?,
    };
    let source = FileSource::capture_file(&input)?;
    let mut bytes = Vec::new();
    input
        .by_ref()
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err(format!("Local state exceeds its {limit}-byte budget"));
    }
    source.check_file(&input)?;
    Ok((
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())?,
        Some(source),
    ))
}

pub(super) fn lock(directory: &File, name: &str) -> Result<File, String> {
    let path = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd())).join(name);
    let lock = match file_source::open_regular_file(&path, false) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            file_source::create_file_in(&path, &[], directory)?
        }
        Err(error) => return Err(error.to_string()),
    };
    // Worker-only and nonblocking: all cooperating windows read/merge under the
    // same retained-directory lock. Failure leaves the previous state intact.
    lock.try_lock()
        .map_err(|e| format!("Could not lock local state: {e}"))?;
    FileSource::capture_file(&lock)?.check_file(&lock)?;
    Ok(lock)
}

pub(super) fn update<T: Default + DeserializeOwned + Serialize>(
    path: &Path,
    lock_name: &str,
    limit: usize,
    validate: impl Fn(&T) -> Result<(), String>,
    change: impl FnOnce(&mut T) -> Result<(), String>,
) -> Result<T, String> {
    let parent = path.parent().ok_or("No local state directory")?;
    let directory =
        kernel_interface::create_content_directory(parent).map_err(|e| e.to_string())?;
    let _lock = lock(&directory, lock_name)?;
    let held = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let target = held.join(path.file_name().ok_or("No state filename")?);
    let (mut state, source) = read(&target, limit)?;
    validate(&state)?;
    change(&mut state)?;
    validate(&state)?;
    let bytes = serde_json::to_vec_pretty(&state).map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err(format!("Local state exceeds its {limit}-byte budget"));
    }
    if let Some(source) = source {
        source.write_in(source.path(), &bytes, &directory)?;
    } else {
        file_source::create_file_in(&target, &bytes, &directory)?;
    }
    Ok(state)
}
