use std::ffi::OsStr;
use std::path::{Path, PathBuf};

pub(super) use little_clip_core::file_inventory::{
    Directory, Entry, Identity, check_revision, format_size, identity, read_directory, read_entry,
    read_pinned_directory,
};

pub(super) use little_clip_core::path_address::{decode_path, display_path};

pub(super) fn parse_address(text: &str, current: &Path, home: &Path) -> Result<PathBuf, String> {
    if text.is_empty() {
        return Err("Enter a directory path".into());
    }
    let path = decode_path(text)?;
    if path == Path::new("~") {
        return Ok(home.into());
    }
    if let Ok(suffix) = path.strip_prefix(OsStr::new("~")) {
        return Ok(home.join(suffix));
    }
    Ok(if path.is_absolute() {
        path
    } else {
        current.join(path)
    })
}

pub(super) use little_clip_core::file_source::{rename_at_noreplace, rename_noreplace};
