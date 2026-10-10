use std::ffi::{CString, OsStr, OsString};
use std::hash::{Hash, Hasher};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Local};

#[derive(Debug)]
pub(super) struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub directory: bool,
    pub regular_file: bool,
    pub typescript: bool,
    pub symlink: bool,
    pub hidden: bool,
    pub size: String,
    pub modified: String,
    pub warning: Option<String>,
    pub revision: u64,
    pub identity: Option<Identity>,
}

impl Entry {
    pub(super) fn is_video(&self) -> bool {
        self.regular_file
            && !self.typescript
            && little_clip_core::animation::is_video_path(&self.path)
    }
    pub(super) fn is_playable_media(&self) -> bool {
        self.regular_file
            && (self.is_video()
                || super::midi_strip::is_midi(&self.path)
                || little_clip_core::audio::is_audio_path(&self.path))
    }
    pub(super) fn is_scene(&self) -> bool {
        self.regular_file && little_clip_scene::is_splat_file(&self.path)
    }
    pub(super) fn is_photo(&self) -> bool {
        self.regular_file
            && (little_clip_core::item::is_image_path(&self.path)
                || little_clip_core::raw_image::is_raw_image_path(&self.path))
    }
}

#[derive(Debug)]
pub(super) struct Directory {
    pub path: PathBuf,
    pub entries: Vec<Entry>,
    pub skipped: usize,
    pub signature: u64,
}

// Called exclusively by the bounded executor. Canonical locations make symlink
// navigation and ancestor revelation agree without traversing cycles in the UI.
pub(super) fn read_directory(path: &Path, cancelled: &AtomicBool) -> Result<Directory, String> {
    let path = path.canonicalize().map_err(|error| error.to_string())?;
    let reader = std::fs::read_dir(&path).map_err(|error| error.to_string())?;
    let mut entries = Vec::new();
    let mut skipped = 0;
    for child in reader {
        if cancelled.load(Ordering::Relaxed) {
            return Err("Directory read cancelled".into());
        }
        match child {
            Ok(child) => entries.push(read_entry(child)),
            Err(_) => skipped += 1,
        }
    }
    entries.sort_by(|a, b| {
        b.directory
            .cmp(&a.directory)
            .then_with(|| a.path.file_name().cmp(&b.path.file_name()))
    });
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for entry in &entries {
        entry.path.hash(&mut hasher);
        entry.revision.hash(&mut hasher);
    }
    skipped.hash(&mut hasher);
    Ok(Directory {
        signature: hasher.finish(),
        path,
        entries,
        skipped,
    })
}

fn read_entry(child: std::fs::DirEntry) -> Entry {
    let path = child.path();
    let symlink = child.file_type().is_ok_and(|kind| kind.is_symlink());
    let metadata = std::fs::metadata(&path);
    let directory = metadata.as_ref().is_ok_and(|data| data.is_dir());
    let regular_file = metadata.as_ref().is_ok_and(|data| data.is_file());
    let modified = metadata.as_ref().ok().and_then(|data| data.modified().ok());
    let warning = metadata.as_ref().err().map(|error| error.to_string());
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    if let Ok(data) = &metadata {
        identity(data).hash(&mut hasher);
    }
    let link_metadata = std::fs::symlink_metadata(&path);
    let entry_identity = link_metadata.as_ref().ok().map(identity);
    entry_identity.hash(&mut hasher);
    Entry {
        typescript: regular_file && super::text_strip::is_typescript_source(&path),
        revision: hasher.finish(),
        identity: entry_identity,
        hidden: child.file_name().as_bytes().starts_with(b"."),
        name: display_path(Path::new(&child.file_name())),
        path,
        directory,
        regular_file,
        symlink,
        size: if directory {
            "—".into()
        } else {
            metadata
                .as_ref()
                .map(|data| format_size(data.len()))
                .unwrap_or_else(|_| "—".into())
        },
        modified: modified
            .map(|time| {
                DateTime::<Local>::from(time)
                    .format("%Y-%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_else(|| "—".into()),
        warning,
    }
}

fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

// A reversible address representation: valid Unicode is preserved, backslashes
// are doubled, and invalid bytes/control characters are written as \xHH.
pub(super) fn display_path(path: &Path) -> String {
    let mut result = String::new();
    let mut bytes = path.as_os_str().as_bytes();
    while !bytes.is_empty() {
        let (valid, invalid) = match std::str::from_utf8(bytes) {
            Ok(text) => (text, 0),
            Err(error) => (
                std::str::from_utf8(&bytes[..error.valid_up_to()]).unwrap_or_default(),
                error
                    .error_len()
                    .unwrap_or(bytes.len() - error.valid_up_to()),
            ),
        };
        for ch in valid.chars() {
            if ch == '\\' {
                result.push_str("\\\\");
            } else if ch.is_control() {
                let mut encoded = [0; 4];
                for byte in ch.encode_utf8(&mut encoded).as_bytes() {
                    result.push_str(&format!("\\x{byte:02X}"));
                }
            } else {
                result.push(ch);
            }
        }
        bytes = &bytes[valid.len()..];
        for byte in &bytes[..invalid] {
            result.push_str(&format!("\\x{byte:02X}"));
        }
        bytes = &bytes[invalid..];
    }
    result
}

pub(super) fn decode_path(text: &str) -> Result<PathBuf, String> {
    let mut decoded = Vec::new();
    let mut bytes = text.as_bytes().iter().copied();
    while let Some(byte) = bytes.next() {
        if byte != b'\\' {
            decoded.push(byte);
            continue;
        }
        match bytes.next() {
            Some(b'\\') => decoded.push(b'\\'),
            Some(b'x') => {
                let hi = bytes.next().and_then(|b| (b as char).to_digit(16));
                let lo = bytes.next().and_then(|b| (b as char).to_digit(16));
                let (Some(hi), Some(lo)) = (hi, lo) else {
                    return Err("Invalid path escape: use \\xHH or \\\\".into());
                };
                decoded.push((hi * 16 + lo) as u8);
            }
            _ => return Err("Invalid path escape: use \\xHH or \\\\".into()),
        }
    }
    if decoded.contains(&0) {
        return Err("A path cannot contain a NUL byte".into());
    }
    Ok(PathBuf::from(OsString::from_vec(decoded)))
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Identity(u64, u64, u64, i64, i64, i64, i64, u32);

pub(super) fn identity(data: &std::fs::Metadata) -> Identity {
    Identity(
        data.dev(),
        data.ino(),
        data.len(),
        data.mtime(),
        data.mtime_nsec(),
        data.ctime(),
        data.ctime_nsec(),
        data.mode(),
    )
}

pub(super) fn rename_noreplace(source: &Path, destination: &Path) -> std::io::Result<()> {
    rename_at_noreplace(
        libc::AT_FDCWD,
        source.as_os_str(),
        libc::AT_FDCWD,
        destination.as_os_str(),
    )
}

pub(super) fn rename_at_noreplace(
    source_fd: i32,
    source: &OsStr,
    destination_fd: i32,
    destination: &OsStr,
) -> std::io::Result<()> {
    let source = CString::new(source.as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let destination = CString::new(destination.as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    // SAFETY: both names are NUL-terminated and each descriptor is borrowed from
    // a live directory or is AT_FDCWD. Linux refuses any existing destination.
    let result = unsafe {
        libc::renameat2(
            source_fd,
            source.as_ptr(),
            destination_fd,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

impl Identity {
    // Moving the directory entry changes ctime; object identity, contents, mode
    // and mtime must still match before removing a quarantined move source.
    pub(super) fn after_rename_matches(self, actual: Self) -> bool {
        (self.0, self.1, self.2, self.3, self.4, self.7)
            == (actual.0, actual.1, actual.2, actual.3, actual.4, actual.7)
    }
}

// Shared enumeration identity guard for metadata and video preview workers.
pub(super) fn check_revision(path: &Path, revision: u64) -> Result<(), String> {
    let mut version = std::collections::hash_map::DefaultHasher::new();
    identity(&std::fs::metadata(path).map_err(|e| e.to_string())?).hash(&mut version);
    Some(identity(
        &std::fs::symlink_metadata(path).map_err(|e| e.to_string())?,
    ))
    .hash(&mut version);
    if version.finish() != revision {
        return Err("File changed since enumeration".into());
    }
    Ok(())
}
