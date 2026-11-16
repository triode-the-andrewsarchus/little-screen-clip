//! Streaming tree inventories; no whole-tree path or identity allocation.
use crate::{
    Result, cancelled,
    root::{Root, anchor},
    store::Store,
};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, ReadDir},
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Entry {
    #[serde(with = "little_clip_core::path_serde")]
    pub path: PathBuf,
    pub kind: String,
    pub size: u64,
    pub modified: i64,
    pub nanos: i64,
    pub mode: u32,
    pub device: u64,
    pub inode: u64,
    pub hash: Option<String>,
    #[serde(with = "little_clip_core::path_serde::optional")]
    pub link: Option<PathBuf>,
}
#[derive(Clone)]
pub(crate) struct Limits {
    pub entries: u64,
    pub bytes: u64,
    pub depth: usize,
    pub timeout: Duration,
    pub hashes: bool,
    pub exclusions: Vec<PathBuf>,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            entries: u64::MAX,
            bytes: u64::MAX,
            depth: 256,
            timeout: Duration::MAX,
            hashes: true,
            exclusions: vec![],
        }
    }
}
pub(crate) struct Coverage {
    pub count: u64,
    pub complete: bool,
    pub message: String,
}
struct Frame {
    relative: PathBuf,
    _pin: File,
    reader: ReadDir,
}
impl Frame {
    fn new(root: &Root, relative: PathBuf) -> Result<Self> {
        let pin = root.directory(&relative, false)?;
        let reader = fs::read_dir(anchor(&pin))?;
        Ok(Self {
            relative,
            _pin: pin,
            reader,
        })
    }
}
pub(crate) fn scan(
    db: &Store,
    root: &Root,
    id: &str,
    limits: &Limits,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> Result<Coverage> {
    db.clear_scan(id)?;
    let mut stack = vec![Frame::new(root, PathBuf::new())?];
    let started = Instant::now();
    let mut count = 0;
    let mut bytes = 0_u64;
    let mut complete = true;
    let mut message = String::new();
    while let Some(frame) = stack.last_mut() {
        cancelled(cancel)?;
        if count >= limits.entries || bytes >= limits.bytes || started.elapsed() >= limits.timeout {
            complete = false;
            message = "Partial: catalog entry, byte or time budget reached".into();
            break;
        }
        let Some(child) = frame.reader.next() else {
            stack.pop();
            continue;
        };
        let child = child?;
        let relative = frame.relative.join(child.file_name());
        let excluded = limits.exclusions.iter().any(|p| relative.starts_with(p));
        let metadata = fs::symlink_metadata(child.path())?;
        let mut entry = Entry {
            path: relative.clone(),
            kind: if metadata.is_dir() {
                "directory"
            } else if metadata.is_file() {
                "file"
            } else if metadata.file_type().is_symlink() {
                "symlink"
            } else {
                "special"
            }
            .into(),
            size: metadata.len(),
            modified: metadata.mtime(),
            nanos: metadata.mtime_nsec(),
            mode: metadata.mode(),
            device: metadata.dev(),
            inode: metadata.ino(),
            hash: None,
            link: None,
        };
        if excluded {
            entry.kind = "excluded".into();
        } else if metadata.file_type().is_symlink() {
            entry.link = Some(fs::read_link(child.path())?);
        } else if metadata.is_file() && limits.hashes {
            entry.hash = Some(root.digest(&relative, cancel)?);
        } else if !metadata.is_dir() && !metadata.is_file() {
            complete = false;
            message = "Partial: special files were not read".into();
        }
        if entry.kind == "directory" {
            if stack.len() >= limits.depth {
                entry.kind = "excluded".into();
                complete = false;
                message = "Partial: directory depth limit".into();
            } else {
                stack.push(Frame::new(root, relative.clone())?);
            }
        }
        let json = serde_json::to_string(&entry)?;
        bytes = bytes
            .saturating_add((json.len() + relative.as_os_str().as_bytes().len() * 3 + 256) as u64);
        let parent = relative
            .parent()
            .unwrap_or(Path::new(""))
            .as_os_str()
            .as_bytes();
        db.0.execute(
            "INSERT INTO entries VALUES(?1,?2,?3,?4,?5)",
            params![
                id,
                relative.as_os_str().as_bytes(),
                parent,
                relative.file_name().unwrap_or_default().as_bytes(),
                json
            ],
        )?;
        count += 1;
        if count % 128 == 0 {
            root.validate()?;
            progress(count, bytes);
        }
    }
    root.validate()?;
    progress(count, bytes);
    if message.is_empty() {
        message = if limits.exclusions.is_empty() {
            "Complete"
        } else {
            "Complete with configured exclusions"
        }
        .into();
    }
    Ok(Coverage {
        count,
        complete,
        message,
    })
}
pub(crate) fn entries(db: &Store, scan: &str, after: &[u8], limit: usize) -> Result<Vec<Entry>> {
    let mut statement =
        db.0.prepare("SELECT json FROM entries WHERE scan=?1 AND path>?2 ORDER BY path LIMIT ?3")?;
    let rows = statement.query_map(params![scan, after, limit as i64], |r| {
        r.get::<_, String>(0)
    })?;
    rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
}
pub(crate) fn entry(db: &Store, scan: &str, path: &Path) -> Result<Option<Entry>> {
    use rusqlite::OptionalExtension;
    let json: Option<String> =
        db.0.query_row(
            "SELECT json FROM entries WHERE scan=?1 AND path=?2",
            params![scan, path.as_os_str().as_bytes()],
            |r| r.get(0),
        )
        .optional()?;
    Ok(json.map(|s| serde_json::from_str(&s)).transpose()?)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_catalog_excludes_subtrees_without_reading_payloads() {
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        fs::create_dir(&data).unwrap();
        fs::create_dir(data.join("huge")).unwrap();
        fs::write(data.join("huge/secret"), b"secret").unwrap();
        fs::write(data.join("file"), b"hello").unwrap();
        let db = Store::open(&temp.path().join("db")).unwrap();
        let root = Root::open(&data).unwrap();
        let cancel = AtomicBool::new(false);
        let limits = Limits {
            hashes: false,
            exclusions: vec!["huge".into()],
            ..Default::default()
        };
        let coverage = scan(&db, &root, "cat", &limits, &cancel, |_, _| {}).unwrap();
        assert!(coverage.complete);
        assert_eq!(coverage.count, 2);
        assert_eq!(
            entry(&db, "cat", Path::new("huge")).unwrap().unwrap().kind,
            "excluded"
        );
        assert!(
            entry(&db, "cat", Path::new("huge/secret"))
                .unwrap()
                .is_none()
        );
        assert!(
            entry(&db, "cat", Path::new("file"))
                .unwrap()
                .unwrap()
                .hash
                .is_none()
        );
        let coverage = scan(
            &db,
            &root,
            "cap",
            &Limits {
                entries: 1,
                ..limits
            },
            &cancel,
            |_, _| {},
        )
        .unwrap();
        assert!(!coverage.complete);
    }
}
