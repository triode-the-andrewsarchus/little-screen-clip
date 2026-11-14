//! Guarded state-file snapshots and atomic, cooperating-writer updates.
use super::{Change, State};
use little_clip_core::{
    file_source::{self, FileSource},
    kernel_interface,
};
use std::{
    collections::BTreeSet,
    fs::File,
    io::Read,
    os::{fd::AsRawFd, unix::ffi::OsStrExt},
    path::{Component, Path, PathBuf},
};

const LIMIT: usize = 1024 * 1024;
const FOLDERS: usize = 4096;

pub(super) fn read(path: &Path) -> Result<(State, Option<FileSource>), String> {
    let mut input = match file_source::open_regular_file(path, false) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((State::default(), None));
        }
        result => result.map_err(|e| e.to_string())?,
    };
    let source = FileSource::capture_file(&input)?;
    let mut bytes = Vec::new();
    input
        .by_ref()
        .take(LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > LIMIT {
        return Err("Folder preferences exceed 1 MiB".into());
    }
    source.check_file(&input)?;
    let mut state: State = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    validate(&state)?;
    state.folders.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((state, Some(source)))
}
fn validate(state: &State) -> Result<(), String> {
    if state.version != 1 {
        return Err("Unsupported folder-preference version; file left unchanged".into());
    }
    if state.folders.len() > FOLDERS {
        return Err("Folder preferences exceed 4,096 folders".into());
    }
    let mut paths = BTreeSet::new();
    for record in &state.folders {
        let path = &record.path;
        if !path.is_absolute()
            || path.as_os_str().as_bytes().len() > 4096
            || path.as_os_str().as_bytes().contains(&0)
            || path.components().any(|c| matches!(c, Component::ParentDir))
            || !paths.insert(path)
        {
            return Err("Invalid or duplicate folder-preference path".into());
        }
    }
    Ok(())
}
pub(super) fn lock(directory: &File) -> Result<File, String> {
    let path = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()))
        .join(".folder-views.lock");
    let lock = match file_source::open_regular_file(&path, false) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            file_source::create_file_in(&path, &[], directory)?
        }
        Err(error) => return Err(error.to_string()),
    };
    // Advisory locking is metadata-only, nonblocking and worker-owned. Other
    // Files windows merge their own change into the latest state under this lock.
    lock.try_lock()
        .map_err(|e| format!("Could not lock folder preferences: {e}"))?;
    FileSource::capture_file(&lock)?.check_file(&lock)?;
    Ok(lock)
}
pub(super) fn save(path: &Path, change: &Change) -> Result<State, String> {
    let parent = path
        .parent()
        .ok_or("No folder-preference state directory")?;
    let directory =
        kernel_interface::create_content_directory(parent).map_err(|e| e.to_string())?;
    let _lock = lock(&directory)?;
    let held = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let target = held.join(path.file_name().ok_or("No preference filename")?);
    let (mut state, source) = read(&target)?;
    state.update(&change.path, change.preference);
    validate(&state)?;
    let bytes = serde_json::to_vec_pretty(&state).map_err(|e| e.to_string())?;
    if bytes.len() > LIMIT {
        return Err("Folder preferences exceed 1 MiB".into());
    }
    if let Some(source) = source {
        source.write_in(source.path(), &bytes, &directory)?;
    } else {
        file_source::create_file_in(&target, &bytes, &directory)?;
    }
    Ok(state)
}
