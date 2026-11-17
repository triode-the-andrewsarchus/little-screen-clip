//! Immutable metadata catalogs exposed through a read-only, bounded FUSE process.
use crate::{
    Catalog, DriveConfig, DriveRule, Result,
    backup::{self, Backup},
    discovery,
    root::Root,
    scan::{self, Entry, Limits},
    store::Store,
};
use fuser::{FileAttr, FileType, INodeNo};
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
    time::Duration,
};
mod capture_details;
pub(crate) mod redaction;
mod server;

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
        detailed_coverage: true,
        hash_bytes: u64::MAX,
        entries: config.catalog_max_entries,
        bytes: config.catalog_max_bytes,
        depth: config.catalog_max_depth,
        timeout: Duration::from_secs(config.catalog_timeout_seconds),
        hashes: false,
        access_dates: false,
        exclusions,
        reuse_scan: None,
    };
    let (cat_db, pages) = capture_details::database(&path, config.catalog_max_bytes)?;
    let coverage = scan::scan(&cat_db, root, "catalog", &limits, cancel, progress)?;
    cat_db.0.pragma_update(None, "max_page_count", pages)?;
    let details = capture_details::source(root, coverage.directory, coverage.stopped)?;
    let catalog = Catalog {
        id: id.clone(),
        drive: drive.id.clone(),
        evidence: drive.evidence.clone(),
        label: drive.label.clone(),
        captured: crate::now(),
        entries: coverage.count,
        complete: coverage.complete,
        coverage: coverage.message,
        details: Some(details),
        redacted: None,
        redaction_generation: 0,
        pending_redactions: 0,
        redaction_error: None,
        root_excluded: false,
        mount: None,
    };
    cat_db.put("source", "drive", drive)?;
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
    fn saved_entry(&self, ino: u64) -> Result<Option<Vec<u8>>> {
        if ino == 1 {
            return Ok(None);
        }
        let entry = self
            .row(ino)?
            .ok_or_else(|| std::io::Error::from_raw_os_error(libc::ENOENT))?;
        let bytes = serde_json::to_vec(&entry)?;
        if bytes.len() > 65536 {
            return Err(std::io::Error::from_raw_os_error(libc::E2BIG).into());
        }
        Ok(Some(bytes))
    }
    fn coverage(
        &self,
        ino: u64,
    ) -> Result<Option<little_clip_core::catalog_metadata::CatalogEntryStatus>> {
        if ino == 1 {
            return Ok(if self.catalog.root_excluded {
                Some(little_clip_core::catalog_metadata::CatalogEntryStatus::NotCatalogued)
            } else if self
                .catalog
                .details
                .as_ref()
                .is_some_and(|details| !details.directory.subtree_complete)
            {
                Some(little_clip_core::catalog_metadata::CatalogEntryStatus::Incomplete)
            } else {
                None
            });
        }
        let entry = self.row(ino)?.ok_or("Unknown catalog entry")?;
        Ok(if entry.kind == "excluded" {
            Some(little_clip_core::catalog_metadata::CatalogEntryStatus::NotCatalogued)
        } else if entry.kind == "special" {
            Some(
                little_clip_core::catalog_metadata::CatalogEntryStatus::MetadataOnly(
                    little_clip_core::catalog_metadata::CatalogSpecialKind::from_mode(entry.mode),
                ),
            )
        } else if entry
            .directory_coverage
            .as_ref()
            .is_some_and(|coverage| !coverage.subtree_complete)
        {
            Some(little_clip_core::catalog_metadata::CatalogEntryStatus::Incomplete)
        } else {
            None
        })
    }
    fn details(&self, ino: u64) -> Result<Option<Vec<u8>>> {
        let Some(source) = &self.catalog.details else {
            return Ok(None);
        };
        let (directory, failure, excluded) = if ino == 1 {
            (
                Some(source.directory.clone()),
                None,
                self.catalog.root_excluded,
            )
        } else {
            let entry = self.row(ino)?.ok_or("Unknown catalog entry")?;
            (
                entry.directory_coverage,
                entry.failure,
                entry.kind == "excluded",
            )
        };
        let directory = directory.map(|mut coverage| {
            if excluded {
                coverage = Default::default();
            } else if !coverage.listing_complete && coverage.issue.is_none() {
                coverage.issue = source.stopped;
            }
            coverage
        });
        Ok(Some(serde_json::to_vec(&serde_json::json!({
            "directory": directory, "failure": failure, "excluded": excluded,
            "filesystem": source.filesystem, "mtime_resolution_ns": source.mtime_resolution_ns,
            "source_mount": source.source_mount, "namespace": source.namespace,
        }))?))
    }
    fn listing_complete(&self, ino: u64) -> Result<bool> {
        if ino == 1 {
            return Ok(self
                .catalog
                .details
                .as_ref()
                .map_or(self.catalog.complete, |details| {
                    details.directory.listing_complete
                }));
        }
        let entry = self.row(ino)?.ok_or("Unknown catalog directory")?;
        Ok(entry
            .directory_coverage
            .map_or(self.catalog.complete, |coverage| coverage.listing_complete))
    }
    fn row(&self, ino: u64) -> Result<Option<Entry>> {
        let Some(id) = i64::try_from(ino)
            .ok()
            .and_then(|ino| ino.checked_sub(2))
            .filter(|id| *id > 0)
        else {
            return Ok(None);
        };
        let json: Option<String> = self
            .db
            .0
            .query_row(
                "SELECT json FROM entries WHERE scan='catalog' AND rowid=?1",
                [id],
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
        row.map(|(id, json)| Ok((catalog_inode(id)?, serde_json::from_str(&json)?)))
            .transpose()
    }
    fn directory_page(&self, ino: u64, offset: i64) -> Result<Vec<(u64, OsString, Entry)>> {
        if offset < 0 {
            return Err(std::io::Error::from_raw_os_error(libc::EINVAL).into());
        }
        let path = self.parent_path(ino)?;
        let mut statement = self.db.0.prepare("SELECT rowid,name,json FROM entries WHERE scan='catalog' AND parent=?1 AND rowid>?2 ORDER BY rowid LIMIT 512")?;
        let rows = statement.query_map(
            params![path.as_os_str().as_bytes(), offset.saturating_sub(2)],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?;
        let page = rows
            .map(|row| {
                let (id, name, json) = row?;
                Ok((
                    catalog_inode(id)?,
                    OsString::from_vec(name),
                    serde_json::from_str(&json)?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        if page.is_empty() && !self.listing_complete(ino)? {
            return Err(std::io::Error::from_raw_os_error(libc::EIO).into());
        }
        Ok(page)
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
    fn directory_inode(&self, path: &Path) -> Result<u64> {
        if path.as_os_str().is_empty() {
            return Ok(1);
        }
        match self.lookup_path(path)? {
            Some((ino, entry)) if entry.kind == "directory" => Ok(ino),
            _ => Err(std::io::Error::from_raw_os_error(libc::EIO).into()),
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
    fn attr(&self, ino: u64, entry: Option<&Entry>) -> Result<FileAttr> {
        let clock = |seconds: u64| {
            i64::try_from(seconds)
                .ok()
                .and_then(|seconds| backup::time(seconds, 0))
                .ok_or_else(|| std::io::Error::from_raw_os_error(libc::EOVERFLOW))
        };
        let time = match entry {
            Some(entry) => backup::time(entry.modified, entry.nanos)
                .ok_or_else(|| std::io::Error::from_raw_os_error(libc::EOVERFLOW))?,
            None => clock(self.catalog.captured)?,
        };
        let revision = clock(self.catalog.redacted.unwrap_or(self.catalog.captured))?
            .checked_add(Duration::from_nanos(self.catalog.redaction_generation))
            .ok_or_else(|| std::io::Error::from_raw_os_error(libc::EOVERFLOW))?;
        let kind = entry.map_or(FileType::Directory, |e| file_type(&e.kind));
        Ok(FileAttr {
            ino: INodeNo(ino),
            size: entry.map_or(0, |e| e.size),
            blocks: 0,
            atime: time,
            mtime: time,
            // Source mtime stays intact. Virtual ctime identifies this metadata
            // snapshot so a reopened/redacted mount cannot reuse old previews.
            ctime: revision,
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
        })
    }
}
fn catalog_inode(rowid: i64) -> Result<u64> {
    // 1 is the virtual root and 2 is the directory-cookie prefix. Real entry
    // IDs must also fit Linux's signed seek offsets so every page can resume.
    let inode = rowid
        .checked_add(2)
        .filter(|inode| *inode > 2)
        .ok_or_else(|| std::io::Error::from_raw_os_error(libc::EOVERFLOW))?;
    Ok(inode as u64)
}
fn file_type(kind: &str) -> FileType {
    match kind {
        "directory" | "excluded" => FileType::Directory,
        "symlink" => FileType::Symlink,
        _ => FileType::RegularFile,
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
    server::mount(
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
        id,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    mod coverage_tests;
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
        assert_eq!(
            catalog.details.as_ref().unwrap().stopped.unwrap().reason,
            little_clip_core::catalog_metadata::CatalogScanReason::Database
        );
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
        assert_eq!(fs.attr(ino, Some(&entry)).unwrap().size, 15);
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
        let server = filesystem(&fixture.state.path, catalog.clone());
        let opens = server.payload_opens.clone();
        let session = server::spawn(server, &mount, &catalog.id).unwrap();
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
