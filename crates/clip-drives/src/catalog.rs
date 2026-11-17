//! Immutable metadata catalogs exposed through a read-only, bounded FUSE process.
use crate::{
    Catalog, DriveConfig, DriveRule, Result,
    backup::{self, Backup},
    discovery,
    root::Root,
    scan::{self, Entry, Limits},
    store::Store,
};
use fuser::{
    FileAttr, FileType, Filesystem, ReplyAttr, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry,
    ReplyOpen, ReplyXattr, Request,
};
use little_clip_core::file_source::FileSource;
use rusqlite::{OptionalExtension, params};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    fs::File,
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::FileExt,
    },
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::{Duration, SystemTime},
};
pub(crate) mod redaction;

pub(crate) fn capture(
    db: &Store,
    state: &Root,
    drive: &crate::Drive,
    root: &Root,
    config: &DriveConfig,
    cancel: &AtomicBool,
    progress: impl FnMut(u64, u64),
) -> Result<Catalog> {
    root.validate()?;
    let id = uuid::Uuid::new_v4().to_string();
    let catalogs = state.directory(Path::new("catalogs"), true)?;
    let path = crate::root::anchor(&catalogs).join(format!("{id}.sqlite"));
    let mut exclusions = Vec::new();
    {
        let transaction = db.0.unchecked_transaction()?;
        db.visit::<DriveRule>("rule", |rule| {
            if rule.drive == drive.id && !rule.catalog {
                if rule.relative.as_os_str().is_empty() {
                    return Err("Cataloging is disabled for this drive".into());
                }
                exclusions.push(rule.relative);
            }
            Ok(())
        })?;
        transaction.commit()?;
    }
    let limits = Limits {
        hash_bytes: u64::MAX,
        entries: config.catalog_max_entries,
        bytes: config.catalog_max_bytes,
        depth: config.catalog_max_depth,
        timeout: Duration::from_secs(config.catalog_timeout_seconds),
        hashes: false,
        exclusions,
        reuse_scan: None,
    };
    let cat_db = Store::open(&path)?;
    if config.catalog_max_bytes < 128 * 1024 {
        return Err("Catalog byte limit must be at least 128 KiB".into());
    }
    cat_db.0.execute_batch("PRAGMA journal_mode=DELETE")?;
    let page_size: u64 = cat_db.0.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    let pages = config.catalog_max_bytes / page_size;
    cat_db
        .0
        .pragma_update(None, "max_page_count", pages.saturating_sub(16))?;
    let coverage = scan::scan(&cat_db, root, "catalog", &limits, cancel, progress)?;
    cat_db.0.pragma_update(None, "max_page_count", pages)?;
    let catalog = Catalog {
        id: id.clone(),
        drive: drive.id.clone(),
        evidence: drive.evidence.clone(),
        label: drive.label.clone(),
        captured: crate::now(),
        entries: coverage.count,
        complete: coverage.complete,
        coverage: coverage.message,
        redacted: None,
        redaction_generation: 0,
        pending_redactions: 0,
        redaction_error: None,
        root_excluded: false,
        mount: None,
    };
    cat_db.put("catalog", &id, &catalog)?;
    redaction::publish(db, &catalog, &limits.exclusions)?;
    Ok(catalog)
}
struct OpenFile {
    file: File,
    source: FileSource,
    root: Root,
}
impl OpenFile {
    fn validated(root: Root, file: File, entry: &Entry) -> Result<Self> {
        use std::os::unix::fs::MetadataExt;

        let source = FileSource::capture_file(&file)?;
        let metadata = file.metadata()?;
        if metadata.len() != entry.size
            || metadata.mtime() != entry.modified
            || metadata.mtime_nsec() != entry.nanos
        {
            return Err("Catalog provider revision is stale".into());
        }
        // Hash the retained provider handle, never a second lookup of its name.
        if let Some(hash) = &entry.hash
            && source.sha256(&file, &AtomicBool::new(false), Duration::MAX)? != *hash
        {
            return Err("Catalog provider contents are stale".into());
        }
        source.check_file(&file)?;
        root.validate()?;
        Ok(Self { file, source, root })
    }
}
struct CatalogFs {
    db: Store,
    state: PathBuf,
    catalog: Catalog,
    opened: HashMap<u64, OpenFile>,
    next_handle: u64,
    #[cfg(test)]
    payload_opens: std::sync::Arc<std::sync::atomic::AtomicU64>,
    _lease: redaction::Lease,
}
impl CatalogFs {
    fn coverage(
        &self,
        ino: u64,
    ) -> Result<Option<little_clip_core::catalog_metadata::CatalogEntryStatus>> {
        if ino == 1 {
            return Ok(self
                .catalog
                .root_excluded
                .then_some(little_clip_core::catalog_metadata::CatalogEntryStatus::NotCatalogued));
        }
        let entry = self.row(ino)?.ok_or("Unknown catalog entry")?;
        Ok((entry.kind == "excluded")
            .then_some(little_clip_core::catalog_metadata::CatalogEntryStatus::NotCatalogued))
    }
    fn row(&self, ino: u64) -> Result<Option<Entry>> {
        let json: Option<String> = self
            .db
            .0
            .query_row(
                "SELECT json FROM entries WHERE scan='catalog' AND rowid=?1",
                [(ino as i64) - 2],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json.map(|s| serde_json::from_str(&s)).transpose()?)
    }
    fn lookup_path(&self, path: &Path) -> Result<Option<(u64, Entry)>> {
        let row: Option<(i64, String)> = self
            .db
            .0
            .query_row(
                "SELECT rowid,json FROM entries WHERE scan='catalog' AND path=?1",
                [path.as_os_str().as_bytes()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        row.map(|(id, json)| Ok((id as u64 + 2, serde_json::from_str(&json)?)))
            .transpose()
    }
    fn parent_path(&self, ino: u64) -> Result<PathBuf> {
        if ino == 1 {
            if self.catalog.root_excluded {
                return Err("Directory was not catalogued".into());
            }
            Ok(PathBuf::new())
        } else {
            let entry = self.row(ino)?.ok_or("Unknown catalog directory")?;
            if entry.kind == "excluded" {
                return Err("Directory was not catalogued".into());
            }
            if entry.kind != "directory" {
                return Err("Not a catalog directory".into());
            }
            Ok(entry.path)
        }
    }
    fn resolve(&self, entry: &Entry) -> Result<OpenFile> {
        let state_db = Store::existing(&self.state.join("drives.sqlite"), true)?;
        let mut providers = Vec::new();
        for drive in discovery::discover().unwrap_or_default() {
            if drive.id == self.catalog.drive
                && drive.evidence == self.catalog.evidence
                && !drive.ambiguous
                && let Some(mount) = drive.mount
            {
                providers.push((mount, None));
            }
        }
        let mut offset = 0;
        loop {
            let versions: Vec<Backup> = state_db.list("backup", offset, 128)?;
            if versions.is_empty() {
                break;
            }
            for version in versions {
                if version.drive == self.catalog.drive && version.evidence == self.catalog.evidence
                {
                    if version.kind == crate::BackupKind::Files {
                        if version.identity.is_some() {
                            providers.push((version.path, version.identity));
                        }
                    } else if let Some(mount) = version.mount {
                        providers.push((mount, None));
                    }
                }
            }
            offset += 128;
            if providers.len() >= 128 {
                break;
            }
        }
        let catalog_mounts = crate::mounts::inventory()?
            .into_iter()
            .filter(|mount| {
                mount
                    .source
                    .as_os_str()
                    .as_bytes()
                    .starts_with(b"little-clip-catalog:")
            })
            .map(|mount| mount.target)
            .collect::<Vec<_>>();
        let managed_mounts = self.state.join("mounts");
        let recursive = |path: &Path| {
            path.starts_with(&managed_mounts)
                || catalog_mounts.iter().any(|mount| path.starts_with(mount))
        };
        let mut stale = false;
        for (path, identity) in providers {
            // Reject our own FUSE paths before opening even their directory metadata.
            if recursive(&path) {
                continue;
            }
            let Ok(root) = Root::open(&path) else {
                continue;
            };
            if recursive(&root.path) {
                continue;
            }
            if let Some(identity) = identity
                && root.storage_identity().ok().as_ref() != Some(&identity)
            {
                stale = true;
                continue;
            }
            let Ok(file) = root.input(&entry.path) else {
                continue;
            };
            match OpenFile::validated(root, file, entry) {
                Ok(open) => return Ok(open),
                Err(_) => stale = true,
            }
        }
        Err(if stale {
            "Catalog entry is stale; refresh its catalog"
        } else {
            "File contents unavailable: insert the drive or make a matching backup available"
        }
        .into())
    }
    fn attr(&self, ino: u64, entry: Option<&Entry>) -> FileAttr {
        let time = entry
            .and_then(|e| backup::time(e.modified, e.nanos))
            .unwrap_or(SystemTime::UNIX_EPOCH + Duration::from_secs(self.catalog.captured));
        let kind = entry.map_or(FileType::Directory, |e| file_type(&e.kind));
        FileAttr {
            ino,
            size: entry.map_or(0, |e| e.size),
            blocks: 0,
            atime: time,
            mtime: time,
            // Source mtime stays intact. Virtual ctime identifies this metadata
            // snapshot so a reopened/redacted mount cannot reuse old previews.
            ctime: SystemTime::UNIX_EPOCH
                + Duration::from_secs(self.catalog.redacted.unwrap_or(self.catalog.captured))
                + Duration::from_nanos(self.catalog.redaction_generation),
            crtime: time,
            kind,
            perm: if kind == FileType::Directory {
                0o555
            } else {
                0o444
            },
            nlink: if kind == FileType::Directory { 2 } else { 1 },
            uid: rustix::process::getuid().as_raw(),
            gid: rustix::process::getgid().as_raw(),
            rdev: 0,
            blksize: 4096,
            flags: 0,
        }
    }
}
fn file_type(kind: &str) -> FileType {
    match kind {
        "directory" | "excluded" => FileType::Directory,
        "symlink" => FileType::Symlink,
        _ => FileType::RegularFile,
    }
}
impl Filesystem for CatalogFs {
    fn getxattr(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        name: &OsStr,
        size: u32,
        reply: ReplyXattr,
    ) {
        if name != OsStr::new(little_clip_core::catalog_metadata::CATALOG_STATUS_XATTR) {
            reply.error(libc::ENODATA);
            return;
        }
        match self.coverage(ino) {
            Ok(Some(status)) => xattr_reply(status.token(), size, reply),
            Ok(None) => reply.error(libc::ENODATA),
            _ => reply.error(libc::ENOENT),
        }
    }
    fn listxattr(&mut self, _req: &Request<'_>, ino: u64, size: u32, reply: ReplyXattr) {
        match self.coverage(ino) {
            Ok(Some(_)) => {
                let key = format!(
                    "{}\0",
                    little_clip_core::catalog_metadata::CATALOG_STATUS_XATTR
                );
                xattr_reply(key.as_bytes(), size, reply);
            }
            Ok(None) => xattr_reply(b"", size, reply),
            Err(_) => reply.error(libc::ENOENT),
        }
    }
    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let result = (|| {
            let parent = self.parent_path(parent)?;
            let path = parent.join(name);
            if !little_clip_core::drives::valid_relative(&path) {
                return Err("Invalid catalog path".into());
            }
            self.lookup_path(&path)
        })();
        match result {
            Ok(Some((_, entry))) if entry.kind == "unavailable" => reply.error(libc::EIO),
            Ok(Some((ino, entry))) => {
                reply.entry(&Duration::from_secs(1), &self.attr(ino, Some(&entry)), 0)
            }
            Ok(None) => reply.error(libc::ENOENT),
            Err(_) => reply.error(libc::EACCES),
        }
    }
    fn getattr(&mut self, _req: &Request<'_>, ino: u64, reply: ReplyAttr) {
        if ino == 1 {
            reply.attr(&Duration::from_secs(1), &self.attr(1, None));
            return;
        }
        match self.row(ino) {
            Ok(Some(e)) if e.kind == "unavailable" => reply.error(libc::EIO),
            Ok(Some(e)) => reply.attr(&Duration::from_secs(1), &self.attr(ino, Some(&e))),
            _ => reply.error(libc::ENOENT),
        }
    }
    fn readlink(&mut self, _req: &Request<'_>, ino: u64, reply: ReplyData) {
        let result = (|| -> Result<PathBuf> {
            let entry = self.row(ino)?.ok_or("Unknown catalog link")?;
            let link = entry.link.ok_or("Not a symlink")?;
            let mut target = entry.path.parent().unwrap_or(Path::new("")).to_owned();
            for component in link.components() {
                match component {
                    std::path::Component::Normal(name) => target.push(name),
                    std::path::Component::CurDir => (),
                    std::path::Component::ParentDir => {
                        if !target.pop() {
                            return Err("Link escapes catalog".into());
                        }
                    }
                    _ => return Err("Absolute links remain metadata-only".into()),
                }
            }
            if self.lookup_path(&target)?.is_none() {
                return Err("Link target is not catalogued".into());
            }
            Ok(link)
        })();
        match result {
            Ok(link) => reply.data(link.as_os_str().as_bytes()),
            Err(_) => reply.error(libc::ENODATA),
        }
    }
    fn opendir(&mut self, _req: &Request<'_>, ino: u64, _flags: i32, reply: ReplyOpen) {
        match self.parent_path(ino) {
            Ok(_) => reply.opened(0, 0),
            Err(_) => reply.error(libc::EACCES),
        }
    }
    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        let result = (|| -> Result<()> {
            let path = self.parent_path(ino)?;
            if offset < 1 && reply.add(ino, 1, FileType::Directory, ".") {
                return Ok(());
            }
            if offset < 2 && reply.add(1, 2, FileType::Directory, "..") {
                return Ok(());
            }
            let mut statement=self.db.0.prepare("SELECT rowid,name,json FROM entries WHERE scan='catalog' AND parent=?1 AND rowid+2>?2 ORDER BY rowid LIMIT 512")?;
            let rows = statement.query_map(params![path.as_os_str().as_bytes(), offset], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (id, name, json) = row?;
                let entry: Entry = serde_json::from_str(&json)?;
                if reply.add(
                    id as u64 + 2,
                    id + 2,
                    file_type(&entry.kind),
                    OsString::from_vec(name),
                ) {
                    break;
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => reply.ok(),
            Err(_) => reply.error(libc::EACCES),
        }
    }
    fn open(&mut self, _req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        #[cfg(test)]
        self.payload_opens
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if flags & libc::O_ACCMODE != libc::O_RDONLY {
            reply.error(libc::EROFS);
            return;
        }
        if self.opened.len() >= 128 {
            reply.error(libc::EMFILE);
            return;
        }
        let result = (|| {
            let entry = self.row(ino)?.ok_or("Unknown entry")?;
            if entry.kind != "file" {
                return Err("Not a regular catalog file".into());
            }
            self.resolve(&entry)
        })();
        match result {
            Ok(open) => {
                self.next_handle = self.next_handle.wrapping_add(1).max(1);
                let handle = self.next_handle;
                self.opened.insert(handle, open);
                reply.opened(handle, 1);
            }
            Err(e) => reply.error(if e.to_string().contains("stale") {
                libc::ESTALE
            } else {
                libc::ENOMEDIUM
            }),
        }
    }
    fn read(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _owner: Option<u64>,
        reply: ReplyData,
    ) {
        let result = (|| -> Result<Vec<u8>> {
            if offset < 0 || size > 1024 * 1024 {
                return Err("Invalid read size".into());
            }
            let open = self.opened.get(&fh).ok_or("Unknown file handle")?;
            open.root.validate()?;
            open.source.check_file(&open.file)?;
            let mut bytes = vec![0; size as usize];
            let count = open.file.read_at(&mut bytes, offset as u64)?;
            open.source.check_file(&open.file)?;
            bytes.truncate(count);
            Ok(bytes)
        })();
        match result {
            Ok(bytes) => reply.data(&bytes),
            Err(_) => reply.error(libc::ESTALE),
        }
    }
    fn release(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        fh: u64,
        _flags: i32,
        _owner: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        self.opened.remove(&fh);
        reply.ok();
    }
}
fn xattr_reply(value: &[u8], size: u32, reply: ReplyXattr) {
    if size == 0 {
        reply.size(value.len() as u32);
    } else if size < value.len() as u32 {
        reply.error(libc::ERANGE);
    } else {
        reply.data(value);
    }
}

fn mount_options(id: &str) -> [fuser::MountOption; 6] {
    [
        fuser::MountOption::RO,
        fuser::MountOption::NoExec,
        fuser::MountOption::NoDev,
        fuser::MountOption::NoSuid,
        fuser::MountOption::Subtype(little_clip_core::catalog_metadata::CATALOG_FS_SUBTYPE.into()),
        fuser::MountOption::FSName(format!("little-clip-catalog:{id}")),
    ]
}
/// Mounts an existing immutable catalog until it is unmounted by its owner.
///
/// # Errors
/// Returns missing-catalog, database, admission or FUSE mount failures.
pub fn mount_catalog(state: &Path, id: &str, mount: &Path) -> Result<()> {
    if uuid::Uuid::parse_str(id).is_err() {
        return Err("Invalid catalog identifier".into());
    }
    let lease = redaction::Lease::acquire(std::sync::Arc::new(Root::open(state)?), id, false)?
        .ok_or("Catalog cleanup is running; try mounting again when it finishes")?;
    let registry = Store::existing(&state.join("drives.sqlite"), true)?;
    let registered: Catalog = registry.get("catalog", id)?.ok_or("Unknown catalog")?;
    if registered.pending_redactions != 0 {
        return Err("Catalog cleanup is pending; unmount its existing view and wait for cleanup before mounting again".into());
    }
    let db = Store::existing(&lease.path, true)?;
    let catalog: Catalog = db.get("catalog", id)?.ok_or("Catalog does not exist")?;
    let root = Root::open(mount)?;
    fuser::mount2(
        CatalogFs {
            _lease: lease,
            db,
            state: state.into(),
            catalog,
            opened: HashMap::new(),
            next_handle: 0,
            #[cfg(test)]
            payload_opens: Default::default(),
        },
        &root.path,
        &mount_options(id),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    mod provider_tests;
    mod redaction_tests;
    use super::*;
    struct Fixture {
        db: Store,
        drive: crate::Drive,
        source: Root,
        state: Root,
        temp: tempfile::TempDir,
    }
    impl Fixture {
        fn capture(&self, config: &DriveConfig) -> Result<Catalog> {
            capture(
                &self.db,
                &self.state,
                &self.drive,
                &self.source,
                config,
                &AtomicBool::new(false),
                |_, _| {},
            )
        }
        fn backup(&self) -> Result<Backup> {
            backup::files(
                &self.db,
                &self.drive,
                &self.source,
                &Root::create(&self.temp.path().join("backups"))?,
                &AtomicBool::new(false),
                |_, _| {},
            )
        }
    }
    fn fixture() -> Fixture {
        let temp = if let Some(base) = std::env::var_os("CLIP_DRIVE_CATALOG_FIXTURE") {
            tempfile::Builder::new()
                .prefix("catalog-scale-")
                .tempdir_in(base)
                .unwrap()
        } else {
            tempfile::tempdir().unwrap()
        };
        let source = temp.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("file.txt"), b"catalog payload").unwrap();
        std::fs::create_dir(source.join("excluded")).unwrap();
        std::fs::write(source.join("excluded/hidden"), b"never catalogued").unwrap();
        let state = temp.path().join("state");
        std::fs::create_dir(&state).unwrap();
        let db = Store::open(&state.join("drives.sqlite")).unwrap();
        let drive = crate::Drive {
            id: "part:catalog-fixture".into(),
            label: "Catalog fixture".into(),
            device: "/dev/not-real".into(),
            mount: Some(source.clone()),
            part_uuid: Some("catalog-fixture".into()),
            filesystem_uuid: None,
            filesystem: "vfat".into(),
            capacity: 1024 * 1024,
            drive_capacity: 1024 * 1024,
            disk_sequence: Some(1),
            partition_layout: None,
            removable: true,
            read_only: false,
            evidence: "fixture".into(),
            ambiguous: false,
        };
        db.put(
            "rule",
            "exclude",
            &DriveRule {
                id: "exclude".into(),
                drive: drive.id.clone(),
                relative: "excluded".into(),
                catalog: false,
                index: false,
            },
        )
        .unwrap();
        Fixture {
            db,
            drive,
            source: Root::open(&source).unwrap(),
            state: Root::open(&state).unwrap(),
            temp,
        }
    }
    fn filesystem(state: &Path, catalog: Catalog) -> CatalogFs {
        CatalogFs {
            _lease: redaction::Lease::acquire(
                std::sync::Arc::new(Root::open(state).unwrap()),
                &catalog.id,
                false,
            )
            .unwrap()
            .unwrap(),
            db: Store::open(
                &state
                    .join("catalogs")
                    .join(format!("{}.sqlite", catalog.id)),
            )
            .unwrap(),
            state: state.into(),
            catalog,
            opened: HashMap::new(),
            next_handle: 0,
            payload_opens: Default::default(),
        }
    }
    #[test]
    #[ignore = "creates one million owned fixture files; select CLIP_DRIVE_CATALOG_FIXTURE explicitly"]
    fn million_file_catalog_exclusion_and_limits_remain_bounded() {
        use std::time::Instant;
        assert!(
            std::env::var_os("CLIP_DRIVE_CATALOG_FIXTURE").is_some(),
            "Select an owned fixture parent"
        );
        let fixture = fixture();
        let started = Instant::now();
        for directory in 0..1000 {
            let path = fixture
                .source
                .path
                .join("excluded")
                .join(format!("d-{directory:04}"));
            std::fs::create_dir(&path).unwrap();
            for file in 0..1000 {
                File::create(path.join(format!("f-{file:04}"))).unwrap();
            }
        }
        let creation = started.elapsed().as_secs_f64();
        let started = Instant::now();
        let excluded = fixture.capture(&DriveConfig::default()).unwrap();
        assert!(excluded.complete);
        assert_eq!(excluded.entries, 2);
        let exclusion = started.elapsed().as_secs_f64();
        fixture.db.remove("rule", "exclude").unwrap();
        let started = Instant::now();
        let limited = fixture
            .capture(&DriveConfig {
                catalog_max_entries: 1000,
                catalog_max_bytes: 1024 * 1024,
                ..Default::default()
            })
            .unwrap();
        assert!(!limited.complete);
        assert!(limited.entries <= 1000);
        let path = fixture
            .state
            .path
            .join("catalogs")
            .join(format!("{}.sqlite", limited.id));
        let bytes = std::fs::metadata(path).unwrap().len();
        assert!(bytes <= 1024 * 1024);
        let peak_rss = std::fs::read_to_string("/proc/self/status")
            .unwrap()
            .lines()
            .find(|l| l.starts_with("VmHWM:"))
            .unwrap_or("unknown")
            .to_owned();
        println!(
            "{}",
            serde_json::json!({"files":1000000,"creation_seconds":creation,"excluded_catalog_seconds":exclusion,"limited_catalog_seconds":started.elapsed().as_secs_f64(),"limited_entries":limited.entries,"catalog_bytes":bytes,"peak_rss":peak_rss})
        );
    }
    #[test]
    fn catalog_database_cap_preserves_committed_partial_coverage() {
        let fixture = fixture();
        for index in 0..2000 {
            File::create(fixture.source.path.join(format!("file-{index:06}"))).unwrap();
        }
        let config = DriveConfig {
            catalog_max_bytes: 128 * 1024,
            ..Default::default()
        };
        let catalog = fixture.capture(&config).unwrap();
        assert!(!catalog.complete);
        let path = fixture
            .state
            .path
            .join("catalogs")
            .join(format!("{}.sqlite", catalog.id));
        assert!(std::fs::metadata(&path).unwrap().len() <= config.catalog_max_bytes);
        let stored = Store::existing(&path, true).unwrap();
        let count: u64 = stored
            .0
            .query_row("SELECT count(*) FROM entries", [], |r| r.get(0))
            .unwrap();
        assert_eq!(catalog.entries, count);
        assert!(count < 2000);
    }
    #[test]
    fn catalog_refuses_replacements_of_admitted_source_and_state_roots() {
        for replace_source in [true, false] {
            let fixture = fixture();
            let path = if replace_source {
                &fixture.source.path
            } else {
                &fixture.state.path
            };
            std::fs::rename(path, path.with_extension("original")).unwrap();
            std::fs::create_dir(path).unwrap();
            std::fs::write(path.join("canary"), b"unrelated replacement").unwrap();
            let error = fixture.capture(&DriveConfig::default()).unwrap_err();
            assert!(error.to_string().contains("replaced or removed"), "{error}");
            assert!(!fixture.state.path.join("catalogs").exists());
            assert_eq!(
                std::fs::read(path.join("canary")).unwrap(),
                b"unrelated replacement"
            );
            assert!(
                fixture
                    .db
                    .list::<Catalog>("catalog", 0, 10)
                    .unwrap()
                    .is_empty()
            );
        }
    }
    #[test]
    fn metadata_listing_works_offline_and_matching_backup_supplies_content() {
        let fixture = fixture();
        let catalog = fixture.capture(&DriveConfig::default()).unwrap();
        let fs = filesystem(&fixture.state.path, catalog);
        let (ino, entry) = fs.lookup_path(Path::new("file.txt")).unwrap().unwrap();
        assert_eq!(fs.attr(ino, Some(&entry)).size, 15);
        assert!(fs.resolve(&entry).is_err());
        assert!(
            fs.lookup_path(Path::new("excluded/hidden"))
                .unwrap()
                .is_none()
        );
        fixture.backup().unwrap();
        let open = fs.resolve(&entry).unwrap();
        let mut bytes = [0; 15];
        open.file.read_exact_at(&mut bytes, 0).unwrap();
        assert_eq!(&bytes, b"catalog payload");
    }
    #[test]
    #[ignore = "requires /dev/fuse and fusermount3"]
    fn real_fuse_mount_lists_offline_metadata_and_returns_media_errors() {
        let fixture = fixture();
        std::fs::create_dir(fixture.source.path.join("empty")).unwrap();
        std::fs::write(
            fixture.source.path.join("metadata-only.ts"),
            b"const value: number = 1;",
        )
        .unwrap();
        let catalog = fixture.capture(&DriveConfig::default()).unwrap();
        let mount = fixture.temp.path().join("mount");
        std::fs::create_dir(&mount).unwrap();
        let options = mount_options(&catalog.id);
        let server = filesystem(&fixture.state.path, catalog);
        let opens = server.payload_opens.clone();
        let session = fuser::spawn_mount2(server, &mount, &options).unwrap();
        assert_eq!(std::fs::metadata(mount.join("file.txt")).unwrap().len(), 15);
        assert!(std::fs::read(mount.join("file.txt")).is_err());
        assert!(std::fs::read_dir(mount.join("excluded")).is_err());
        opens.store(0, std::sync::atomic::Ordering::Relaxed);
        let entries =
            little_clip_core::file_inventory::read_directory(&mount, &AtomicBool::new(false))
                .unwrap();
        assert_eq!(
            opens.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "Metadata listing must not open TypeScript or other file payloads"
        );
        let excluded = entries
            .entries
            .iter()
            .find(|entry| entry.name == "excluded")
            .unwrap();
        assert_eq!(
            excluded.catalog_status,
            Some(little_clip_core::catalog_metadata::CatalogEntryStatus::NotCatalogued)
        );
        assert_eq!(excluded.size, "Not catalogued");
        little_clip_core::file_inventory::check_revision(&excluded.path, excluded.revision)
            .unwrap();
        let empty = entries
            .entries
            .iter()
            .find(|entry| entry.name == "empty")
            .unwrap();
        assert_eq!(empty.catalog_status, None);
        assert!(
            std::fs::read_dir(mount.join("empty"))
                .unwrap()
                .next()
                .is_none()
        );
        let error = little_clip_core::file_inventory::read_directory(
            &mount.join("excluded"),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(error.starts_with("Not catalogued:"), "{error}");
        let pin = little_clip_core::kernel_interface::pin(&mount.join("excluded"), true).unwrap();
        let pinned_error = little_clip_core::file_inventory::read_pinned_directory(
            &mount.join("excluded"),
            &pin,
            &AtomicBool::new(false),
            100,
        )
        .unwrap_err();
        assert!(
            pinned_error.starts_with("Not catalogued:"),
            "{pinned_error}"
        );
        drop(pin);
        let view = little_clip_core::catalog_metadata::CatalogMount::detect(&mount).unwrap();
        assert_eq!(
            view.status(&mount.join("excluded")),
            Some(little_clip_core::catalog_metadata::CatalogEntryStatus::NotCatalogued)
        );
        assert_eq!(view.status(fixture.source.path.as_path()), None);
        drop(view);
        let mut backup = fixture.backup().unwrap();
        assert_eq!(
            std::fs::read(mount.join("file.txt")).unwrap(),
            b"catalog payload"
        );
        assert!(std::fs::write(mount.join("new"), b"no").is_err());
        backup.path = mount.clone();
        fixture.db.put("backup", &backup.id, &backup).unwrap();
        assert!(
            std::fs::read(mount.join("file.txt")).is_err(),
            "A catalog must never resolve contents through itself"
        );
        drop(session);
    }
}
