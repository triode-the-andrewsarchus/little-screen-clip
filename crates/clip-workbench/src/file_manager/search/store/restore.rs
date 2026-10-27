//! Import data into our schema; supplied triggers, views and FTS indexes never run.
use super::{
    FTS_SCHEMA, QUERY_INDEXES, SCHEMA_VERSION, Store, create_schema, decode_vector, maintenance,
};
use crate::file_manager::filesystem::{self, Entry};
use rusqlite::{Connection, OpenFlags, config::DbConfig, limits::Limit, params, types::Value};
use std::{
    fs::File,
    io::Read,
    os::{
        fd::AsRawFd,
        unix::{ffi::OsStrExt, fs::FileExt},
    },
    path::{Component, Path},
    sync::atomic::AtomicBool,
};

pub(in crate::file_manager::search) struct PreparedRestore {
    staging: tempfile::NamedTempFile,
    vectors: u64,
}

impl Store {
    pub fn prepare_restore(
        &self,
        path: &Path,
        limits: &little_clip_core::config::FileSearchLimits,
        cancel: &AtomicBool,
    ) -> Result<PreparedRestore, String> {
        let input =
            crate::file_source::open_regular_file(path, false).map_err(|e| e.to_string())?;
        let metadata = input.metadata().map_err(|e| e.to_string())?;
        require_space(&self.owner.directory, metadata.len().saturating_mul(2))?;
        let before = filesystem::identity(&metadata);
        let held = format!("/proc/self/fd/{}", self.owner.directory.as_raw_fd());
        let staging = tempfile::Builder::new()
            .prefix(".search-restore-")
            .tempfile_in(&held)
            .map_err(|e| e.to_string())?;
        let snapshot = tempfile::Builder::new()
            .prefix(".search-restore-")
            .tempfile_in(&held)
            .map_err(|e| e.to_string())?;
        let page_size = self
            .connection
            .query_row("PRAGMA page_size", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let vectors = super::super::reader::prepare_restore(
            [&input, snapshot.as_file(), staging.as_file()],
            limits.max_mib.saturating_mul(1_048_576),
            limits.max_vectors,
            page_size,
            cancel,
        )?;
        if before != filesystem::identity(&input.metadata().map_err(|e| e.to_string())?) {
            return Err("Backup changed while it was being read".into());
        }
        staging.as_file().sync_all().map_err(|e| e.to_string())?;
        Ok(PreparedRestore { staging, vectors })
    }

    /// SQLite's backup transaction retains the old index if preparation or copying fails.
    /// Current enrollment identities take precedence over a historical backup.
    pub fn apply_restore(
        &mut self,
        prepared: PreparedRestore,
        configured: &[std::path::PathBuf],
        cancel: &AtomicBool,
    ) -> Result<(), String> {
        require_space(
            &self.owner.directory,
            prepared
                .staging
                .as_file()
                .metadata()
                .map_err(|e| e.to_string())?
                .len()
                .saturating_mul(2),
        )?;
        let staged =
            Connection::open_with_flags(prepared.staging.path(), OpenFlags::SQLITE_OPEN_READ_WRITE)
                .map_err(|e| e.to_string())?;
        staged
            .execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF;")
            .map_err(|e| e.to_string())?;
        for (_, root) in self.monitor_roots()? {
            if configured.contains(&root.path) {
                staged.execute("INSERT INTO roots(path,device,inode,available,state) VALUES(?1,?2,?3,0,'checking') ON CONFLICT(path) DO UPDATE SET device=excluded.device,inode=excluded.inode,state='checking',available=0,error=NULL",
                    params![root.path.as_os_str().as_bytes(), root.device as i64, root.inode as i64]).map_err(|e| e.to_string())?;
            }
        }
        for path in configured {
            staged
                .execute(
                    "UPDATE roots SET state='checking' WHERE path=?1",
                    [path.as_os_str().as_bytes()],
                )
                .map_err(|e| e.to_string())?;
        }
        // SQLite backup replaces pages without ordinary SQL change counters.
        self.invalidate_vectors()?;
        maintenance::copy_database(
            &staged,
            &mut self.connection,
            cancel,
            Some(&self.storage_fault),
        )?;
        self.vector_count = prepared.vectors;
        Ok(())
    }
}

/// Called only by the disposable reader. Both descriptors are inherited owned handles.
pub(in crate::file_manager::search) fn prepare_in_helper(
    input: &File,
    snapshot: &File,
    output: &File,
    max_bytes: u64,
    max_vectors: u64,
    page_size: u32,
) -> Result<u64, String> {
    check_handles(input, output, max_bytes)?;
    check_handles(input, snapshot, max_bytes)?;
    snapshot_input(input, snapshot, max_bytes)?;
    let source = open_snapshot(snapshot)?;
    let mut target = create_staging(output, max_bytes, page_size)?;
    let vectors = import_records(&source, &mut target, max_vectors)?;
    verify_staging(&target)?;
    target.close().map_err(|(_, e)| e.to_string())?;
    Ok(vectors)
}

fn import_records(
    source: &Connection,
    target: &mut Connection,
    max_vectors: u64,
) -> Result<u64, String> {
    let tx = target.transaction().map_err(|e| e.to_string())?;
    let mut vectors = 0;
    let legacy: bool = source
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .map_err(|e| e.to_string())?
        == 4;
    for table in TABLES {
        let table = if legacy && table.name == "files" {
            &LEGACY_FILES
        } else {
            table
        };
        copy_table(source, &tx, table, max_vectors, &mut vectors)?;
    }
    tx.execute_batch("UPDATE roots SET available=0,state='disabled',error=NULL; UPDATE memberships SET verified=0; UPDATE directories SET complete=0;").map_err(|e| e.to_string())?;
    validate_relations(&tx)?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(vectors)
}

// SQLite resolves descriptor symlinks to paths. Copy through the pinned input
// handle before allowing SQLite to open a private, parent-owned staging name.
fn snapshot_input(input: &File, snapshot: &File, max_bytes: u64) -> Result<(), String> {
    let expected = input.metadata().map_err(|e| e.to_string())?.len();
    let mut reader = input
        .try_clone()
        .map_err(|e| e.to_string())?
        .take(max_bytes.saturating_add(1));
    let mut writer = snapshot.try_clone().map_err(|e| e.to_string())?;
    let copied = std::io::copy(&mut reader, &mut writer).map_err(|e| e.to_string())?;
    if copied != expected || copied > max_bytes {
        return Err("Backup changed or exceeded the storage limit while copying".into());
    }
    check_header(snapshot, max_bytes)
}

fn verify_staging(target: &Connection) -> Result<(), String> {
    let integrity: String = target
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if integrity != "ok" {
        return Err(format!("Invalid rebuilt index: {integrity}"));
    }
    target
        .execute_batch("INSERT INTO segment_fts(segment_fts,rank) VALUES('integrity-check',1);")
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn check_handles(input: &File, output: &File, max_bytes: u64) -> Result<(), String> {
    for file in [input, output] {
        if !file.metadata().map_err(|e| e.to_string())?.is_file()
            || little_clip_core::kernel_interface::classify_handle(file)
                .map_err(|e| e.to_string())?
                .protected()
        {
            return Err("Restore requires ordinary regular files".into());
        }
    }
    if output.metadata().map_err(|e| e.to_string())?.len() != 0 {
        return Err("Restore staging file must be empty".into());
    }
    check_header(input, max_bytes)?;
    Ok(())
}

fn open_snapshot(input: &File) -> Result<Connection, String> {
    let source = Connection::open_with_flags(
        format!(
            "file:/proc/self/fd/{}?mode=ro&immutable=1",
            input.as_raw_fd()
        ),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| e.to_string())?;
    source.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA query_only=ON; PRAGMA mmap_size=0; PRAGMA cache_size=-8192;").map_err(|e| e.to_string())?;
    source
        .set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)
        .map_err(|e| e.to_string())?;
    for (limit, value) in [
        (Limit::SQLITE_LIMIT_LENGTH, 1_048_576),
        (Limit::SQLITE_LIMIT_SQL_LENGTH, 65536),
        (Limit::SQLITE_LIMIT_COLUMN, 64),
        (Limit::SQLITE_LIMIT_EXPR_DEPTH, 100),
        (Limit::SQLITE_LIMIT_VDBE_OP, 100000),
        (Limit::SQLITE_LIMIT_ATTACHED, 0),
        (Limit::SQLITE_LIMIT_WORKER_THREADS, 0),
    ] {
        source.set_limit(limit, value).map_err(|e| e.to_string())?;
    }
    let version: u32 = source
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if version != 4 && i64::from(version) != SCHEMA_VERSION {
        return Err("Choose a native version-4 or version-5 index backup".into());
    }
    Ok(source)
}

fn create_staging(output: &File, max_bytes: u64, page_size: u32) -> Result<Connection, String> {
    if !(512..=65536).contains(&page_size) || !page_size.is_power_of_two() {
        return Err("Unsupported index page size".into());
    }
    let target = Connection::open_with_flags(
        format!("/proc/self/fd/{}", output.as_raw_fd()),
        OpenFlags::SQLITE_OPEN_READ_WRITE,
    )
    .map_err(|e| e.to_string())?;
    target.execute_batch(&format!("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA foreign_keys=ON; PRAGMA mmap_size=0; PRAGMA cache_size=-8192; PRAGMA page_size={page_size}; PRAGMA max_page_count={};", max_bytes / u64::from(page_size))).map_err(|e| e.to_string())?;
    create_schema(&target).map_err(|e| e.to_string())?;
    target
        .execute_batch(QUERY_INDEXES)
        .map_err(|e| e.to_string())?;
    target
        .execute_batch(FTS_SCHEMA)
        .map_err(|e| e.to_string())?;
    Ok(target)
}

/// Called after acquiring the sole writer lock, before accepting new work.
/// Only our private scratch namespace is retired; symlinks are never followed.
pub(super) fn clean_staging(directory: &Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    let entries = std::fs::read_dir(directory).map_err(|e| e.to_string())?;
    for entry in entries.take(4096) {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry
            .file_name()
            .as_bytes()
            .starts_with(b".search-restore-")
        {
            continue;
        }
        let metadata = std::fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?;
        // SAFETY: geteuid takes no arguments and has no side effects.
        let owned = metadata.uid() == unsafe { libc::geteuid() };
        if metadata.is_file() && owned && metadata.mode() & 0o777 == 0o600 {
            std::fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn require_space(directory: &File, reserve: u64) -> Result<(), String> {
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: fstatvfs initializes this buffer on success; directory stays owned.
    if unsafe { libc::fstatvfs(directory.as_raw_fd(), stats.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: the successful syscall initialized every field.
    let stats = unsafe { stats.assume_init() };
    let available = stats.f_bavail.saturating_mul(stats.f_frsize);
    if available < reserve.saturating_add(8 * 1024 * 1024) {
        return Err("Not enough free space to stage and transactionally restore the index".into());
    }
    Ok(())
}

pub(super) fn read_header(input: &File) -> Result<[u8; 100], String> {
    let mut header = [0; 100];
    input
        .read_exact_at(&mut header, 0)
        .map_err(|e| e.to_string())?;
    if &header[..16] != b"SQLite format 3\0" {
        return Err("Invalid SQLite database header".into());
    }
    Ok(header)
}

fn check_header(input: &File, max_bytes: u64) -> Result<(), String> {
    let size = input.metadata().map_err(|e| e.to_string())?.len();
    if size < 100 || size > max_bytes {
        return Err("Backup exceeds the index storage limit or is incomplete".into());
    }
    let header = read_header(input)?;
    if header[18..20] != [1, 1] {
        return Err("Choose a completed index backup, not an active WAL database".into());
    }
    let encoded = u16::from_be_bytes([header[16], header[17]]);
    let page = if encoded == 1 {
        65536
    } else {
        u64::from(encoded)
    };
    if !(512..=65536).contains(&page) || !page.is_power_of_two() || size % page != 0 {
        return Err("Invalid backup page layout".into());
    }
    Ok(())
}

struct Table {
    name: &'static str,
    columns: &'static str,
    types: &'static str,
    limit: u64,
}
const LEGACY_FILES: Table = Table {
    name: "files",
    columns: "id,device,inode,revision,pipeline,metadata,state,coverage,error,extraction_version",
    types: "iiiitttnni",
    limit: 2000000,
};
const TABLES: &[Table] = &[
    Table {
        name: "roots",
        columns: "id,path,device,inode,generation,available,state,error",
        types: "ibiiiitn",
        limit: 10000,
    },
    Table {
        name: "files",
        columns: "id,device,inode,revision,pipeline,metadata,state,coverage,error,extraction_version,scope",
        types: "iiiitttnnit",
        limit: 2000000,
    },
    Table {
        name: "paths",
        columns: "path,parent,file_id",
        types: "bbi",
        limit: 4000000,
    },
    Table {
        name: "memberships",
        columns: "root_id,path,generation,verified",
        types: "ibii",
        limit: 8000000,
    },
    Table {
        name: "directories",
        columns: "root_id,path,generation,complete",
        types: "ibii",
        limit: 1000000,
    },
    Table {
        name: "segments",
        columns: "id,file_id,modality,location,text,input_hash,pipeline,vector",
        types: "iitttbtv",
        limit: 100000000,
    },
];

fn check_table(source: &Connection, table: &Table) -> Result<(), String> {
    let (kind, sql): (String, String) = source
        .query_row(
            "SELECT type,sql FROM sqlite_schema WHERE name=?1",
            [table.name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| e.to_string())?;
    if kind != "table"
        || !sql
            .trim_start()
            .to_ascii_uppercase()
            .starts_with("CREATE TABLE ")
    {
        return Err(format!("{} must be an ordinary table", table.name));
    }
    let mut statement = source
        .prepare(&format!("PRAGMA table_xinfo({})", table.name))
        .map_err(|e| e.to_string())?;
    let columns = statement
        .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, i32>(6)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut names = columns
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    let mut expected = table.columns.split(',').collect::<Vec<_>>();
    names.sort_unstable();
    expected.sort_unstable();
    if names != expected || columns.iter().any(|(_, hidden)| *hidden != 0) {
        return Err(format!("Unsupported columns in {}", table.name));
    }
    Ok(())
}

fn copy_table(
    source: &Connection,
    target: &Connection,
    table: &Table,
    max_vectors: u64,
    vectors: &mut u64,
) -> Result<(), String> {
    check_table(source, table)?;
    let mut statement = source
        .prepare(&format!(
            "SELECT {} FROM {} NOT INDEXED",
            table.columns, table.name
        ))
        .map_err(|e| e.to_string())?;
    let placeholders = vec!["?"; table.types.len()].join(",");
    let mut insert = target
        .prepare(&format!(
            "INSERT INTO {}({}) VALUES({placeholders})",
            table.name, table.columns
        ))
        .map_err(|e| e.to_string())?;
    let mut rows = statement.query([]).map_err(|e| e.to_string())?;
    let cap = if table.name == "segments" {
        max_vectors
            .saturating_mul(2)
            .saturating_add(1024)
            .min(table.limit)
    } else {
        table.limit
    };
    let mut count = 0;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        count += 1;
        if count > cap {
            return Err(format!("Too many {} records in backup", table.name));
        }
        let mut values = (0..table.types.len())
            .map(|i| row.get::<_, Value>(i))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        validate_values(table, &mut values)?;
        if table.name == "segments" && matches!(values[7], Value::Blob(_)) {
            *vectors += 1;
            if *vectors > max_vectors {
                return Err("Backup exceeds the configured vector limit".into());
            }
        }
        insert
            .execute(rusqlite::params_from_iter(&values))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn validate_values(table: &Table, values: &mut [Value]) -> Result<(), String> {
    for (kind, value) in table.types.bytes().zip(values.iter()) {
        let valid = matches!(
            (kind, value),
            (b'i', Value::Integer(_))
                | (b't' | b'n', Value::Text(_))
                | (b'b' | b'v', Value::Blob(_))
                | (b'n' | b'v', Value::Null)
        );
        if !valid {
            return Err(format!("Invalid record type in {}", table.name));
        }
    }
    match table.name {
        "roots" => {
            positive(&values[0])?;
            path_value(&values[1])?;
            nonnegative(&values[4])?;
            flag(&values[5])?;
        }
        "files" => validate_file(values)?,
        "paths" => {
            let path = path_value(&values[0])?;
            let parent = path_value(&values[1])?;
            if path.parent() != Some(parent) {
                return Err("Invalid file parent in backup".into());
            }
            positive(&values[2])?;
        }
        "memberships" | "directories" => {
            positive(&values[0])?;
            path_value(&values[1])?;
            nonnegative(&values[2])?;
            flag(&values[3])?;
        }
        "segments" => validate_segment(values)?,
        _ => unreachable!(),
    }
    Ok(())
}
fn positive(value: &Value) -> Result<(), String> {
    if matches!(value, Value::Integer(1..)) {
        Ok(())
    } else {
        Err("Invalid backup identifier".into())
    }
}
fn nonnegative(value: &Value) -> Result<(), String> {
    if matches!(value, Value::Integer(0..)) {
        Ok(())
    } else {
        Err("Invalid backup generation".into())
    }
}
fn flag(value: &Value) -> Result<(), String> {
    if matches!(value, Value::Integer(0 | 1)) {
        Ok(())
    } else {
        Err("Invalid backup flag".into())
    }
}
fn path_value(value: &Value) -> Result<&Path, String> {
    let Value::Blob(bytes) = value else {
        return Err("Invalid path encoding".into());
    };
    let path = Path::new(std::ffi::OsStr::from_bytes(bytes));
    if bytes.len() > 8192
        || bytes.contains(&0)
        || !path.is_absolute()
        || path.components().any(|c| matches!(c, Component::ParentDir))
    {
        return Err("Invalid absolute path in backup".into());
    }
    Ok(path)
}
fn validate_file(values: &mut [Value]) -> Result<(), String> {
    positive(&values[0])?;
    nonnegative(&values[9])?;
    let Value::Text(metadata) = &values[5] else {
        unreachable!()
    };
    if metadata.len() > 65536 {
        return Err("Oversized file metadata".into());
    }
    let entry: Entry =
        serde_json::from_str(metadata).map_err(|e| format!("Invalid file metadata: {e}"))?;
    let object = entry.identity.map(|identity| identity.object());
    if !entry.regular_file
        || entry.directory
        || entry.symlink
        || entry.system.is_some_and(|k| k.protected())
        || object != Some((integer(&values[1]) as u64, integer(&values[2]) as u64))
        || entry.revision != integer(&values[3]) as u64
    {
        return Err("Backup file identity does not match its metadata".into());
    }
    path_value(&Value::Blob(entry.path.as_os_str().as_bytes().to_vec()))?;
    if values.len() == 11 {
        // Keep contexts distinct without accepting a backup's source authority.
        let Value::Text(scope) = &values[10] else {
            unreachable!()
        };
        if scope.len() > 256 {
            return Err("Oversized file source context".into());
        }
        values[10] = Value::Text(format!("restored:{}", integer(&values[0])));
    }
    Ok(())
}
fn integer(value: &Value) -> i64 {
    let Value::Integer(value) = value else {
        unreachable!()
    };
    *value
}
fn validate_segment(values: &[Value]) -> Result<(), String> {
    positive(&values[0])?;
    positive(&values[1])?;
    let Value::Text(modality) = &values[2] else {
        unreachable!()
    };
    if !matches!(modality.as_str(), "text" | "image" | "audio") {
        return Err("Unsupported backup modality".into());
    }
    for index in [3, 4, 6] {
        let Value::Text(text) = &values[index] else {
            unreachable!()
        };
        if text.len() > 16384 {
            return Err("Oversized backup segment".into());
        }
    }
    let Value::Blob(hash) = &values[5] else {
        unreachable!()
    };
    if hash.len() != 32 {
        return Err("Invalid segment input hash".into());
    }
    if let Value::Blob(vector) = &values[7] {
        decode_vector(vector)?;
    }
    Ok(())
}
fn validate_relations(target: &Connection) -> Result<(), String> {
    let mut check = target
        .prepare("PRAGMA foreign_key_check")
        .map_err(|e| e.to_string())?;
    if check
        .query([])
        .map_err(|e| e.to_string())?
        .next()
        .map_err(|e| e.to_string())?
        .is_some()
    {
        return Err("Broken backup references".into());
    }
    let mut query = target.prepare("SELECT m.path,r.path FROM memberships m JOIN roots r ON r.id=m.root_id UNION ALL SELECT d.path,r.path FROM directories d JOIN roots r ON r.id=d.root_id").map_err(|e| e.to_string())?;
    let mut rows = query.query([]).map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let path = row
            .get::<_, Vec<u8>>(0)
            .map(Value::Blob)
            .map_err(|e| e.to_string())?;
        let root = row
            .get::<_, Vec<u8>>(1)
            .map(Value::Blob)
            .map_err(|e| e.to_string())?;
        if !path_value(&path)?.starts_with(path_value(&root)?) {
            return Err("Backup membership escapes its root".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_manager::search::{
        policy::Root,
        store::{DIMENSION, Segment},
    };
    use std::sync::atomic::Ordering;

    struct Fixture {
        _source: tempfile::TempDir,
        _cache: tempfile::TempDir,
        output: tempfile::TempDir,
        store: Store,
        root: std::path::PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let source = tempfile::tempdir().unwrap();
            let cache = tempfile::tempdir().unwrap();
            let output = tempfile::tempdir().unwrap();
            std::fs::write(source.path().join("sample.txt"), "the cat sat").unwrap();
            let root = Root::open(source.path()).unwrap();
            let mut store = Store::open(cache.path()).unwrap();
            let root_id = store.enroll(&root).unwrap();
            let generation = store.begin_scan(root_id).unwrap();
            let entry = filesystem::read_directory(source.path(), &AtomicBool::new(false))
                .unwrap()
                .entries
                .remove(0);
            let (id, _) = store
                .observe(
                    root_id,
                    generation,
                    &entry,
                    &std::fs::metadata(&entry.path).unwrap(),
                    "model",
                )
                .unwrap();
            store
                .publish(
                    id,
                    entry.revision,
                    "model",
                    &[Segment {
                        modality: "text".into(),
                        location: "paragraph".into(),
                        text: "the cat sat".into(),
                        input_hash: vec![1; 32],
                        vector: Some(vec![1.0; DIMENSION]),
                    }],
                    "complete",
                )
                .unwrap();
            store
                .directory_complete(root_id, generation, &root.path)
                .unwrap();
            Self {
                root: root.path,
                _source: source,
                _cache: cache,
                output,
                store,
            }
        }
        fn backup(&self) -> std::path::PathBuf {
            let path = self.output.path().join("backup.sqlite");
            self.store.export(&path, &AtomicBool::new(false)).unwrap();
            path
        }
        fn prepare(&self, path: &Path, max_vectors: u64) -> Result<PreparedRestore, String> {
            let staging = tempfile::NamedTempFile::new_in(self._cache.path()).unwrap();
            let input = File::open(path).unwrap();
            let snapshot = tempfile::NamedTempFile::new_in(self._cache.path()).unwrap();
            let vectors = prepare_in_helper(
                &input,
                snapshot.as_file(),
                staging.as_file(),
                16 * 1024 * 1024,
                max_vectors,
                4096,
            )?;
            Ok(PreparedRestore { staging, vectors })
        }
        fn count(&self, sql: &str) -> u64 {
            self.store
                .connection
                .query_row(sql, [], |r| r.get(0))
                .unwrap()
        }
    }
    #[test]
    fn restore_and_clear_release_warm_vector_buffers() {
        let mut fixture = Fixture::new();
        let backup = fixture.backup();
        let prepared = fixture.prepare(&backup, 1).unwrap();
        let (id, bytes): (i64, Vec<u8>) = fixture
            .store
            .connection
            .query_row("SELECT id,vector FROM segments LIMIT 1", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        let query = vec![1.0; DIMENSION];
        let warm = |store: &Store| {
            store.prepare_vector_cache().unwrap();
            store.score_vector(id, &query, Some(&bytes)).unwrap();
            assert_eq!(store.vector_cache_usage().0, 1);
        };
        warm(&fixture.store);
        fixture
            .store
            .apply_restore(prepared, &[fixture.root.clone()], &AtomicBool::new(false))
            .unwrap();
        assert_eq!(fixture.store.vector_cache_usage(), (0, 0));
        warm(&fixture.store);
        fixture.store.clear_generated().unwrap();
        assert_eq!(fixture.store.vector_cache_usage(), (0, 0));
    }

    #[test]
    fn restore_roundtrip_preserves_vectors_fts_and_current_root_authority() {
        let mut fixture = Fixture::new();
        let backup = fixture.backup();
        let bytes = std::fs::read(&backup).unwrap();
        let prepared = fixture.prepare(&backup, 1).unwrap();
        fixture.store.clear_generated().unwrap();
        // A newer authoritative enrollment must not be replaced by the backup.
        fixture
            .store
            .connection
            .execute_batch("UPDATE roots SET inode=inode+1;")
            .unwrap();
        let inode: i64 = fixture
            .store
            .connection
            .query_row("SELECT inode FROM roots", [], |r| r.get(0))
            .unwrap();
        fixture
            .store
            .apply_restore(prepared, &[fixture.root.clone()], &AtomicBool::new(false))
            .unwrap();
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM segments WHERE vector IS NOT NULL"),
            1
        );
        assert_eq!(fixture.store.vector_count, 1);
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM segment_fts WHERE segment_fts MATCH 'cat'"),
            1
        );
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM roots WHERE available=1"),
            0
        );
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM memberships WHERE verified=1"),
            0
        );
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM directories WHERE complete=1"),
            0
        );
        assert_eq!(
            fixture
                .store
                .connection
                .query_row("SELECT inode FROM roots", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            inode
        );
        assert_eq!(std::fs::read(backup).unwrap(), bytes);
    }
    #[test]
    fn backup_roots_do_not_enroll_into_an_empty_cache() {
        let fixture = Fixture::new();
        let prepared = fixture.prepare(&fixture.backup(), 1).unwrap();
        let cache = tempfile::tempdir().unwrap();
        let mut empty = Store::open(cache.path()).unwrap();
        empty
            .apply_restore(prepared, &[], &AtomicBool::new(false))
            .unwrap();
        assert!(empty.roots().unwrap().is_empty());
        assert!(empty.source_roots().unwrap().is_empty());
        assert_eq!(empty.vector_count, 1);
    }
    #[test]
    fn failed_or_cancelled_restore_keeps_active_records() {
        let mut fixture = Fixture::new();
        let backup = fixture.backup();
        assert!(
            fixture
                .prepare(&backup, 0)
                .err()
                .unwrap()
                .contains("vector limit")
        );
        let prepared = fixture.prepare(&backup, 1).unwrap();
        let cancel = AtomicBool::new(true);
        assert!(fixture.store.apply_restore(prepared, &[], &cancel).is_err());
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM memberships WHERE verified=1"),
            1
        );
        assert_eq!(fixture.store.vector_count, 1);
        cancel.store(false, Ordering::Relaxed);
        // A malformed incoming vector never reaches the active connection.
        let source = Connection::open(&backup).unwrap();
        source
            .execute("UPDATE segments SET vector=?1", [vec![0u8; 1536]])
            .unwrap();
        drop(source);
        assert!(fixture.prepare(&backup, 1).is_err());
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM memberships WHERE verified=1"),
            1
        );
    }
    #[test]
    fn abandoned_staging_is_pruned_only_after_ownership_and_never_follows_links() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let scratch = tempfile::Builder::new()
            .prefix(".search-restore-")
            .tempfile_in(fixture._cache.path())
            .unwrap()
            .keep()
            .unwrap()
            .1;
        let original = fixture._source.path().join("sample.txt");
        let link = fixture._cache.path().join(".search-restore-link");
        symlink(&original, &link).unwrap();
        assert!(Store::open(fixture._cache.path()).is_err());
        assert!(
            scratch.exists(),
            "a second writer must not prune live preparation"
        );
        let Fixture {
            store,
            _cache: cache,
            ..
        } = fixture;
        drop(store);
        let _reopened = Store::open(cache.path()).unwrap();
        assert!(!scratch.exists());
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(original).unwrap(), b"the cat sat");
    }

    #[test]
    fn private_snapshot_reads_the_pinned_backup_after_its_path_is_replaced() {
        let fixture = Fixture::new();
        let path = fixture.backup();
        let input = File::open(&path).unwrap();
        std::fs::rename(&path, fixture.output.path().join("old.sqlite")).unwrap();
        std::fs::write(&path, b"replacement must never be parsed").unwrap();
        let snapshot = tempfile::NamedTempFile::new_in(fixture._cache.path()).unwrap();
        let output = tempfile::NamedTempFile::new_in(fixture._cache.path()).unwrap();
        assert_eq!(
            prepare_in_helper(
                &input,
                snapshot.as_file(),
                output.as_file(),
                16 * 1024 * 1024,
                1,
                4096
            )
            .unwrap(),
            1
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"replacement must never be parsed"
        );
    }

    #[test]
    fn sqlite_full_during_apply_rolls_back_and_retains_the_failure_latch() {
        let mut fixture = Fixture::new();
        let backup = fixture.backup();
        let source = Connection::open(&backup).unwrap();
        source
            .execute("UPDATE segments SET text=?1", ["cat ".repeat(4000)])
            .unwrap();
        drop(source);
        let prepared = fixture.prepare(&backup, 1).unwrap();
        let pages: u64 = fixture
            .store
            .connection
            .query_row("PRAGMA page_count", [], |r| r.get(0))
            .unwrap();
        fixture
            .store
            .connection
            .pragma_update(None, "max_page_count", pages)
            .unwrap();
        let error = fixture
            .store
            .apply_restore(prepared, &[], &AtomicBool::new(false))
            .err()
            .expect("restoring the larger database must hit SQLITE_FULL");
        assert!(error.contains("full"), "{error}");
        assert!(fixture.store.storage_fault.get());
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM memberships WHERE verified=1"),
            1
        );
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM segments WHERE text='the cat sat'"),
            1
        );
        assert_eq!(fixture.store.vector_count, 1);
    }

    #[test]
    fn supplied_triggers_and_fts_are_discarded_and_generated_columns_rejected() {
        let fixture = Fixture::new();
        let backup = fixture.backup();
        let source = Connection::open(&backup).unwrap();
        source.execute_batch("CREATE TRIGGER sabotage AFTER INSERT ON paths BEGIN DELETE FROM roots; END; INSERT INTO segment_fts(segment_fts) VALUES('delete-all');").unwrap();
        drop(source);
        let prepared = fixture.prepare(&backup, 1).unwrap();
        let canonical = Connection::open(prepared.staging.path()).unwrap();
        assert_eq!(
            canonical
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_schema WHERE name='sabotage'",
                    [],
                    |r| r.get::<_, u64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            canonical
                .query_row(
                    "SELECT COUNT(*) FROM segment_fts WHERE segment_fts MATCH 'cat'",
                    [],
                    |r| r.get::<_, u64>(0)
                )
                .unwrap(),
            1
        );
        let source = Connection::open(&backup).unwrap();
        source.execute_batch("ALTER TABLE paths ADD COLUMN surprise TEXT GENERATED ALWAYS AS (hex(path)) VIRTUAL;").unwrap();
        drop(source);
        assert!(fixture.prepare(&backup, 1).is_err());
    }
}
