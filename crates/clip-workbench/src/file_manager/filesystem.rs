use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::{DateTime, Local};

#[derive(Debug)]
pub(super) struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub directory: bool,
    pub symlink: bool,
    pub hidden: bool,
    pub size: String,
    pub modified: String,
    pub warning: Option<String>,
}

#[derive(Debug)]
pub(super) struct Directory {
    pub path: PathBuf,
    pub entries: Vec<Entry>,
    pub skipped: usize,
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
    Ok(Directory {
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
    let modified = metadata.as_ref().ok().and_then(|data| data.modified().ok());
    let warning = metadata.as_ref().err().map(|error| error.to_string());
    Entry {
        hidden: child.file_name().as_bytes().starts_with(b"."),
        name: display_path(Path::new(&child.file_name())),
        path,
        directory,
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

pub(super) fn parse_address(text: &str, current: &Path, home: &Path) -> Result<PathBuf, String> {
    if text.is_empty() {
        return Err("Enter a directory path".into());
    }
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
    let path = PathBuf::from(OsString::from_vec(decoded));
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
