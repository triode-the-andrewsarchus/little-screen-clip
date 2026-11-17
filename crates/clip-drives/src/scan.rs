//! Streaming tree inventories; no whole-tree path or identity allocation.
use crate::{
    Result, cancelled,
    root::{Root, anchor},
    store::Store,
};
use little_clip_core::catalog_metadata::{
    CatalogDirectoryCoverage, CatalogScanIssue, CatalogScanReason, CatalogScanStage,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accessed: Option<(i64, i64)>,
    #[serde(default)]
    pub uid: Option<u32>,
    #[serde(default)]
    pub gid: Option<u32>,
    pub device: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub special_device: Option<u64>,
    pub inode: u64,
    pub hash: Option<String>,
    /// Optional operational scan binding; absent in catalogs and legacy rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash_generation: Option<uuid::Uuid>,
    #[serde(default)]
    pub changed: Option<(i64, i64)>,
    #[serde(default)]
    pub warning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<CatalogScanIssue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory_coverage: Option<CatalogDirectoryCoverage>,
    #[serde(with = "little_clip_core::path_serde::optional")]
    pub link: Option<PathBuf>,
}
impl Entry {
    pub fn from_metadata(relative: &Path, metadata: &fs::Metadata) -> Self {
        Self {
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
            accessed: Some((metadata.atime(), metadata.atime_nsec())),
            uid: Some(metadata.uid()),
            gid: Some(metadata.gid()),
            device: metadata.dev(),
            special_device: matches!(
                metadata.mode() & libc::S_IFMT,
                libc::S_IFCHR | libc::S_IFBLK
            )
            .then(|| metadata.rdev()),
            inode: metadata.ino(),
            hash: None,
            hash_generation: None,
            changed: Some((metadata.ctime(), metadata.ctime_nsec())),
            warning: None,
            failure: None,
            directory_coverage: None,
            link: None,
        }
    }
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
            && self.uid == other.uid
            && self.gid == other.gid
    }
    /// Retains the observation from before our inventory/hash reads changed atime.
    pub fn retain_accessed(&mut self, previous: &Self) {
        if self.device == previous.device && self.inode == previous.inode {
            self.accessed = previous.accessed.or(self.accessed);
        }
    }
}
#[derive(Clone)]
pub(crate) struct Limits {
    pub detailed_coverage: bool,
    pub entries: u64,
    pub bytes: u64,
    pub depth: usize,
    pub timeout: Duration,
    pub hashes: bool,
    pub access_dates: bool,
    pub hash_bytes: u64,
    pub exclusions: Vec<PathBuf>,
    pub reuse_scan: Option<String>,
    pub reuse_generation: Option<uuid::Uuid>,
    pub capture_generation: Option<uuid::Uuid>,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            detailed_coverage: false,
            entries: u64::MAX,
            bytes: u64::MAX,
            depth: 256,
            timeout: Duration::MAX,
            hashes: true,
            access_dates: false,
            hash_bytes: u64::MAX,
            exclusions: vec![],
            reuse_scan: None,
            reuse_generation: None,
            capture_generation: None,
        }
    }
}
pub(crate) struct Coverage {
    pub count: u64,
    pub complete: bool,
    pub message: String,
    pub directory: CatalogDirectoryCoverage,
    pub stopped: Option<CatalogScanIssue>,
}
pub(crate) struct Frame {
    pub relative: PathBuf,
    _pin: File,
    pub reader: ReadDir,
    coverage: CatalogDirectoryCoverage,
}
impl Frame {
    pub fn new(root: &Root, relative: PathBuf) -> Result<Self> {
        let pin = root.directory(&relative, false)?;
        let reader = fs::read_dir(anchor(&pin))?;
        Ok(Self {
            relative,
            _pin: pin,
            reader,
            coverage: CatalogDirectoryCoverage::default(),
        })
    }
    fn validate(&self, root: &Root) -> Result<()> {
        let current = root.directory(&self.relative, false)?.metadata()?;
        let held = self._pin.metadata()?;
        if current.dev() != held.dev() || current.ino() != held.ino() {
            return Err(crate::Error::SourceChanged(
                "Directory was replaced during inventory".into(),
            ));
        }
        Ok(())
    }
}
pub(crate) fn observe(
    root: &Root,
    relative: &Path,
    hashes: bool,
    max_bytes: u64,
    cancel: &AtomicBool,
) -> Result<Option<Entry>> {
    let Some(metadata) = root.metadata(relative)? else {
        return Ok(None);
    };
    let mut entry = Entry::from_metadata(relative, &metadata);
    if metadata.is_file() && hashes {
        entry.hash = Some(root.digest(relative, max_bytes, cancel)?);
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
pub(super) fn unavailable(path: PathBuf, error: &crate::Error, detailed: bool) -> Entry {
    Entry {
        path,
        kind: "unavailable".into(),
        size: 0,
        modified: 0,
        nanos: 0,
        mode: 0,
        accessed: None,
        uid: None,
        gid: None,
        device: 0,
        special_device: None,
        inode: 0,
        hash: None,
        hash_generation: None,
        changed: None,
        warning: Some(error.to_string()),
        failure: detailed.then(|| issue(CatalogScanStage::InspectEntry, error)),
        directory_coverage: None,
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
    remaining: u64,
    cancel: &AtomicBool,
) -> Result<Entry> {
    let excluded = limits
        .exclusions
        .iter()
        .any(|path| relative.starts_with(path));
    let mut entry = observe(root, relative, false, remaining, cancel)?
        .ok_or_else(|| crate::Error::SourceChanged("Entry disappeared during collection".into()))?;
    if limits.hashes && !excluded && entry.kind == "file" && entry.hash.is_none() {
        if entry.size > remaining {
            return Err(
                "RAM logical contents exceed the session hash budget; sparse data retained".into(),
            );
        }
        let cached = limits
            .reuse_scan
            .as_ref()
            .map(|id| self::entry(db, id, relative))
            .transpose()?
            .flatten();
        if let Some(cached) = cached.filter(|old| {
            old.same_revision(&entry)
                && limits
                    .reuse_generation
                    .is_none_or(|generation| old.hash_generation == Some(generation))
        }) {
            entry.hash = cached.hash;
        }
        if entry.hash.is_none() {
            entry = observe(root, relative, true, remaining, cancel)?.ok_or_else(|| {
                crate::Error::SourceChanged("File disappeared during collection".into())
            })?;
        }
    }
    if entry.hash.is_some() {
        entry.hash_generation = limits.capture_generation;
    }
    if excluded {
        entry.kind = "excluded".into();
    }
    if entry.kind == "special" && !limits.detailed_coverage {
        entry.warning = Some("Special file was not read".into());
    }
    if !limits.access_dates {
        entry.accessed = None;
    }
    Ok(entry)
}
mod traversal;
pub(crate) fn scan(
    db: &Store,
    root: &Root,
    id: &str,
    limits: &Limits,
    cancel: &AtomicBool,
    progress: impl FnMut(u64, u64),
) -> Result<Coverage> {
    db.clear_scan(id)?;
    traversal::Scanner::new(db, root, id, limits, cancel)?.run(progress)
}
fn issue(stage: CatalogScanStage, error: &crate::Error) -> CatalogScanIssue {
    let (reason, errno) = match error {
        crate::Error::Io(error) => (CatalogScanReason::Io, error.raw_os_error()),
        crate::Error::SourceChanged(_) => (CatalogScanReason::Changed, None),
        _ => (CatalogScanReason::GuardOrUnknown, None),
    };
    CatalogScanIssue {
        stage,
        reason,
        errno,
    }
}
fn limit(reason: CatalogScanReason) -> CatalogScanIssue {
    CatalogScanIssue {
        stage: CatalogScanStage::Budget,
        reason,
        errno: None,
    }
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
    fn ram_hash_budget_rejects_sparse_growth_before_reading_payloads() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        File::create(source.join("huge-sparse"))
            .unwrap()
            .set_len(10 * 1024 * 1024 * 1024 * 1024)
            .unwrap();
        let db = Store::open(&temp.path().join("scan.sqlite")).unwrap();
        let root = Root::open(&source).unwrap();
        let start = Instant::now();
        let coverage = scan(
            &db,
            &root,
            "ram",
            &Limits {
                hash_bytes: 128 * 1024 * 1024,
                ..Default::default()
            },
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap();
        assert!(!coverage.complete);
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(coverage.message.contains("RAM logical contents"));
        assert_eq!(
            root.metadata(Path::new("huge-sparse"))
                .unwrap()
                .unwrap()
                .len(),
            10 * 1024 * 1024 * 1024 * 1024
        );
    }
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
