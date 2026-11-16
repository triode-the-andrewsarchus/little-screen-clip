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
    ReplyOpen, Request,
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

pub(crate) fn capture(
    db: &Store,
    state: &Path,
    drive: &crate::Drive,
    config: &DriveConfig,
    cancel: &AtomicBool,
    progress: impl FnMut(u64, u64),
) -> Result<Catalog> {
    let root = Root::open(
        drive
            .mount
            .as_deref()
            .ok_or("Mount the drive before saving a catalog")?,
    )?;
    let id = uuid::Uuid::new_v4().to_string();
    let catalogs = Root::create(&state.join("catalogs"))?;
    let path = catalogs.path.join(format!("{id}.sqlite"));
    let rules: Vec<DriveRule> = db.list("rule", 0, 1000)?;
    if rules
        .iter()
        .any(|r| r.drive == drive.id && !r.catalog && r.relative.as_os_str().is_empty())
    {
        return Err("Cataloging is disabled for this drive".into());
    }
    let exclusions = rules
        .into_iter()
        .filter(|r| r.drive == drive.id && !r.catalog)
        .map(|r| r.relative)
        .collect();
    let limits = Limits {
        entries: config.catalog_max_entries,
        bytes: config.catalog_max_bytes,
        depth: config.catalog_max_depth,
        timeout: Duration::from_secs(config.catalog_timeout_seconds),
        hashes: false,
        exclusions,
    };
    let cat_db = Store::open(&path)?;
    let coverage = scan::scan(&cat_db, &root, "catalog", &limits, cancel, progress)?;
    cat_db.0.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    let catalog = Catalog {
        id: id.clone(),
        drive: drive.id.clone(),
        evidence: drive.evidence.clone(),
        label: drive.label.clone(),
        captured: crate::now(),
        entries: coverage.count,
        complete: coverage.complete,
        coverage: coverage.message,
        mount: None,
    };
    cat_db.put("catalog", &id, &catalog)?;
    db.put("catalog", &id, &catalog)?;
    Ok(catalog)
}
struct OpenFile {
    file: File,
    source: FileSource,
    root: Root,
}
struct CatalogFs {
    db: Store,
    state: PathBuf,
    catalog: Catalog,
    opened: HashMap<u64, OpenFile>,
    next_handle: u64,
}
impl CatalogFs {
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
        let state_db = Store::open(&self.state.join("drives.sqlite"))?;
        let mut providers = Vec::new();
        for drive in discovery::discover().unwrap_or_default() {
            if drive.id == self.catalog.drive
                && drive.evidence == self.catalog.evidence
                && !drive.ambiguous
            {
                if let Some(mount) = drive.mount {
                    providers.push(mount);
                }
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
                        providers.push(version.path);
                    } else if let Some(mount) = version.mount {
                        providers.push(mount);
                    }
                }
            }
            offset += 128;
            if providers.len() >= 128 {
                break;
            }
        }
        let mut stale = false;
        for path in providers {
            let Ok(root) = Root::open(&path) else {
                continue;
            };
            let Ok(file) = root.input(&entry.path) else {
                continue;
            };
            use std::os::unix::fs::MetadataExt;
            let m = file.metadata()?;
            if m.len() != entry.size || m.mtime() != entry.modified || m.mtime_nsec() != entry.nanos
            {
                stale = true;
                continue;
            }
            if let Some(hash) = &entry.hash {
                if root.digest(&entry.path, &AtomicBool::new(false))? != *hash {
                    stale = true;
                    continue;
                }
            }
            let source = FileSource::capture_file(&file)?;
            return Ok(OpenFile { file, source, root });
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
            ctime: time,
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
/// Mounts an existing immutable catalog until it is unmounted by its owner.
///
/// # Errors
/// Returns missing-catalog, database, admission or FUSE mount failures.
pub fn mount_catalog(state: &Path, id: &str, mount: &Path) -> Result<()> {
    if uuid::Uuid::parse_str(id).is_err() {
        return Err("Invalid catalog identifier".into());
    }
    let db = Store::open(&state.join("catalogs").join(format!("{id}.sqlite")))?;
    let catalog: Catalog = db.get("catalog", id)?.ok_or("Catalog does not exist")?;
    let root = Root::open(mount)?;
    fuser::mount2(
        CatalogFs {
            db,
            state: state.into(),
            catalog,
            opened: HashMap::new(),
            next_handle: 0,
        },
        &root.path,
        &[
            fuser::MountOption::RO,
            fuser::MountOption::NoExec,
            fuser::MountOption::NoDev,
            fuser::MountOption::NoSuid,
            fuser::MountOption::FSName(format!("little-clip-catalog:{id}")),
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Store, crate::Drive, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
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
            mount: Some(source),
            part_uuid: Some("catalog-fixture".into()),
            filesystem_uuid: None,
            filesystem: "vfat".into(),
            capacity: 1024 * 1024,
            drive_capacity: 1024 * 1024,
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
        (temp, db, drive, state)
    }
    fn filesystem(state: &Path, catalog: Catalog) -> CatalogFs {
        CatalogFs {
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
        }
    }
    #[test]
    fn metadata_listing_works_offline_and_matching_backup_supplies_content() {
        let (temp, db, drive, state) = fixture();
        let cancel = AtomicBool::new(false);
        let catalog = capture(
            &db,
            &state,
            &drive,
            &DriveConfig::default(),
            &cancel,
            |_, _| {},
        )
        .unwrap();
        let fs = filesystem(&state, catalog);
        let (ino, entry) = fs.lookup_path(Path::new("file.txt")).unwrap().unwrap();
        assert_eq!(fs.attr(ino, Some(&entry)).size, 15);
        assert!(fs.resolve(&entry).is_err());
        assert!(
            fs.lookup_path(Path::new("excluded/hidden"))
                .unwrap()
                .is_none()
        );
        backup::files(
            &db,
            &drive,
            &temp.path().join("backups"),
            &cancel,
            |_, _| {},
        )
        .unwrap();
        let open = fs.resolve(&entry).unwrap();
        let mut bytes = [0; 15];
        open.file.read_exact_at(&mut bytes, 0).unwrap();
        assert_eq!(&bytes, b"catalog payload");
    }
    #[test]
    #[ignore = "requires /dev/fuse and fusermount3"]
    fn real_fuse_mount_lists_offline_metadata_and_returns_media_errors() {
        let (temp, db, drive, state) = fixture();
        let catalog = capture(
            &db,
            &state,
            &drive,
            &DriveConfig::default(),
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap();
        let mount = temp.path().join("mount");
        std::fs::create_dir(&mount).unwrap();
        let session = fuser::spawn_mount2(
            filesystem(&state, catalog),
            &mount,
            &[
                fuser::MountOption::RO,
                fuser::MountOption::NoExec,
                fuser::MountOption::NoDev,
                fuser::MountOption::NoSuid,
            ],
        )
        .unwrap();
        assert_eq!(std::fs::metadata(mount.join("file.txt")).unwrap().len(), 15);
        assert!(std::fs::read(mount.join("file.txt")).is_err());
        assert!(std::fs::read_dir(mount.join("excluded")).is_err());
        backup::files(
            &db,
            &drive,
            &temp.path().join("backups"),
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap();
        assert_eq!(
            std::fs::read(mount.join("file.txt")).unwrap(),
            b"catalog payload"
        );
        assert!(std::fs::write(mount.join("new"), b"no").is_err());
        drop(session);
    }
}
