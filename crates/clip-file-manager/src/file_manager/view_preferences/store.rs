//! Folder presentation records over the shared guarded local-state transport.
use super::{Change, State};
use crate::file_manager::local_state;
use little_clip_core::file_source::FileSource;
use std::{
    collections::BTreeSet,
    os::unix::ffi::OsStrExt,
    path::{Component, Path},
};
const FOLDERS: usize = 4096;

pub(super) fn read(path: &Path) -> Result<(State, Option<FileSource>), String> {
    let (mut state, source) = local_state::read::<State>(path, 1024 * 1024)?;
    validate(&state)?;
    state.folders.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((state, source))
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

#[cfg(test)]
pub(super) fn lock(directory: &std::fs::File) -> Result<std::fs::File, String> {
    local_state::lock(directory, ".folder-views.lock")
}
pub(super) fn save(path: &Path, change: &Change) -> Result<State, String> {
    local_state::update(
        path,
        ".folder-views.lock",
        1024 * 1024,
        validate,
        |state: &mut State| {
            state.update(&change.path, change.preference);
            Ok(())
        },
    )
}
