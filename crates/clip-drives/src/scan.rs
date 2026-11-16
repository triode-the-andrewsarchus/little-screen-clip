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
    #[serde(default)]
    pub changed: Option<(i64, i64)>,
    #[serde(default)]
    pub warning: Option<String>,
    #[serde(with = "little_clip_core::path_serde::optional")]
    pub link: Option<PathBuf>,
}
impl Entry {
    pub fn same_revision(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.device == other.device
            && self.inode == other.inode
            && self.size == other.size
            && self.modified == other.modified
            && self.nanos == other.nanos
            && self.changed.is_some()
            && self.changed == other.changed
            && self.mode == other.mode
    }
}
#[derive(Clone)]
pub(crate) struct Limits {
    pub entries: u64,
    pub bytes: u64,
    pub depth: usize,
    pub timeout: Duration,
    pub hashes: bool,
    pub exclusions: Vec<PathBuf>,
    pub reuse_scan: Option<String>,
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
            reuse_scan: None,
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
pub(crate) fn observe(
    root: &Root,
    relative: &Path,
    hashes: bool,
    cancel: &AtomicBool,
) -> Result<Option<Entry>> {
    let Some(metadata) = root.metadata(relative)? else {
        return Ok(None);
    };
    let mut entry = Entry {
        path: relative.into(),
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
        changed: Some((metadata.ctime(), metadata.ctime_nsec())),
        warning: None,
        link: None,
    };
    if metadata.is_file() && hashes {
        entry.hash = Some(root.digest(relative, cancel)?);
        let after = root.metadata(relative)?.ok_or("Hashed file disappeared")?;
        if after.dev() != entry.device
            || after.ino() != entry.inode
            || after.len() != entry.size
            || after.mtime() != entry.modified
            || after.mtime_nsec() != entry.nanos
            || Some((after.ctime(), after.ctime_nsec())) != entry.changed
        {
            return Err("File changed while recording its fingerprint".into());
        }
    }
    if metadata.file_type().is_symlink() {
        let (_parent, path) = root.parent(relative, false)?;
        entry.link = Some(fs::read_link(path)?);
    }
    Ok(Some(entry))
}
fn unavailable(path: PathBuf, error: String) -> Entry {
    Entry {
        path,
        kind: "unavailable".into(),
        size: 0,
        modified: 0,
        nanos: 0,
        mode: 0,
        device: 0,
        inode: 0,
        hash: None,
        changed: None,
        warning: Some(error),
        link: None,
    }
}
pub(crate) fn put(db: &Store, id: &str, entry: &Entry) -> Result<()> {
    db.0.execute("INSERT INTO entries(scan,path,parent,name,json) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(scan,path) DO UPDATE SET json=excluded.json",params![id,entry.path.as_os_str().as_bytes(),entry.path.parent().unwrap_or(Path::new("")).as_os_str().as_bytes(),entry.path.file_name().unwrap_or_default().as_bytes(),serde_json::to_string(entry)?])?;
    Ok(())
}
fn collect_entry(
    db: &Store,
    root: &Root,
    relative: &Path,
    limits: &Limits,
    cancel: &AtomicBool,
) -> Result<Entry> {
    let excluded = limits
        .exclusions
        .iter()
        .any(|path| relative.starts_with(path));
    let mut entry = observe(
        root,
        relative,
        limits.hashes && !excluded && limits.reuse_scan.is_none(),
        cancel,
    )?
    .ok_or("Entry disappeared during collection")?;
    if limits.hashes && !excluded && entry.kind == "file" && entry.hash.is_none() {
        let cached = limits
            .reuse_scan
            .as_ref()
            .map(|id| self::entry(db, id, relative))
            .transpose()?
            .flatten();
        if let Some(cached) = cached.filter(|old| old.same_revision(&entry)) {
            entry.hash = cached.hash;
        }
        if entry.hash.is_none() {
            entry = observe(root, relative, true, cancel)?
                .ok_or("File disappeared during collection")?;
        }
    }
    if excluded {
        entry.kind = "excluded".into();
    }
    if entry.kind == "special" {
        entry.warning = Some("Special file was not read".into());
    }
    Ok(entry)
}
fn descend(root: &Root, entry: &mut Entry, stack: &mut Vec<Frame>, depth: usize) {
    if entry.kind != "directory" {
        return;
    }
    if stack.len() >= depth {
        entry.kind = "excluded".into();
        entry.warning = Some("Directory depth limit".into());
        return;
    }
    match Frame::new(root, entry.path.clone()) {
        Ok(frame) => stack.push(frame),
        Err(error) => entry.warning = Some(error.to_string()),
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
    let mut messages = Vec::new();
    let mut pending = Vec::with_capacity(128);
    while let Some(frame) = stack.last_mut() {
        cancelled(cancel)?;
        let Some(child) = frame.reader.next() else {
            stack.pop();
            continue;
        };
        if count >= limits.entries || bytes >= limits.bytes || started.elapsed() >= limits.timeout {
            complete = false;
            messages.push("catalog entry, byte or time budget reached".to_string());
            break;
        }
        let child = match child {
            Ok(child) => child,
            Err(e) => {
                complete = false;
                messages.push(format!("directory enumeration: {e}"));
                stack.pop();
                continue;
            }
        };
        let relative = frame.relative.join(child.file_name());
        let mut entry = match collect_entry(db, root, &relative, limits, cancel) {
            Ok(entry) => entry,
            Err(error) => {
                cancelled(cancel)?;
                unavailable(relative.clone(), error.to_string())
            }
        };
        descend(root, &mut entry, &mut stack, limits.depth);
        complete &= entry.warning.is_none();
        let json = serde_json::to_string(&entry)?;
        bytes = bytes
            .saturating_add((json.len() + relative.as_os_str().as_bytes().len() * 3 + 256) as u64);
        if bytes > limits.bytes {
            complete = false;
            messages.push("catalog byte budget reached".into());
            break;
        }
        if let Some(warning) = &entry.warning
            && messages.len() < 8
        {
            messages.push(format!("{}: {warning}", relative.display()));
        }
        pending.push(entry);
        count += 1;
        if pending.len() == 128 {
            if !flush(db, id, &pending)? {
                complete = false;
                messages.push("catalog database byte limit reached".into());
                pending.clear();
                count = stored_count(db, id)?;
                break;
            }
            pending.clear();
            root.validate()?;
            progress(count, bytes);
        }
    }
    if !pending.is_empty() && !flush(db, id, &pending)? {
        complete = false;
        messages.push("catalog database byte limit reached".into());
        count = stored_count(db, id)?;
    }
    root.validate()?;
    progress(count, bytes);
    let message = if !complete {
        format!(
            "Partial: {}",
            if messages.is_empty() {
                "incomplete tree".into()
            } else {
                messages.join("; ")
            }
        )
    } else if limits.exclusions.is_empty() {
        "Complete".into()
    } else {
        "Complete with configured exclusions".into()
    };
    Ok(Coverage {
        count,
        complete,
        message,
    })
}
fn flush(db: &Store, id: &str, entries: &[Entry]) -> Result<bool> {
    let transaction = db.0.unchecked_transaction()?;
    for entry in entries {
        match put(db, id, entry) {
            Ok(()) => (),
            Err(crate::Error::Sql(rusqlite::Error::SqliteFailure(error, _)))
                if error.code == rusqlite::ErrorCode::DiskFull =>
            {
                return Ok(false);
            }
            Err(error) => return Err(error),
        }
    }
    transaction.commit()?;
    Ok(true)
}
fn stored_count(db: &Store, id: &str) -> Result<u64> {
    Ok(db
        .0
        .query_row("SELECT count(*) FROM entries WHERE scan=?1", [id], |r| {
            r.get(0)
        })?)
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
/// A bounded keyset cursor that remains valid while other scan rows are updated.
pub(crate) struct Cursor<'a> {
    db: &'a Store,
    scan: &'a str,
    after: Vec<u8>,
    pending: std::collections::VecDeque<Entry>,
    failed: bool,
}
impl<'a> Cursor<'a> {
    pub fn new(db: &'a Store, scan: &'a str) -> Self {
        Self {
            db,
            scan,
            after: Vec::new(),
            pending: Default::default(),
            failed: false,
        }
    }
}
impl Iterator for Cursor<'_> {
    type Item = Result<Entry>;
    fn next(&mut self) -> Option<Self::Item> {
        if let Some(entry) = self.pending.pop_front() {
            return Some(Ok(entry));
        }
        if self.failed {
            return None;
        }
        match entries(self.db, self.scan, &self.after, 128) {
            Ok(batch) => {
                self.after = batch.last()?.path.as_os_str().as_bytes().to_vec();
                self.pending.extend(batch);
                self.pending.pop_front().map(Ok)
            }
            Err(error) => {
                self.failed = true;
                Some(Err(error))
            }
        }
    }
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
