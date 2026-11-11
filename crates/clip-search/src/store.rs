//! Transactional index records. A scan's inventory is separate from content readiness.
mod content;
mod lexical;
mod maintenance;
mod recovery;
use recovery::IndexOwner;
mod fingerprints;
mod pruning;
mod publication;
pub use pruning::PruneCursor;
mod retries;
mod safety;
pub use fingerprints::format as source_format;
pub use publication::Checkpoint;
pub use retries::Retry;
mod restore;
mod session;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
#[cfg(any(test, feature = "test-support"))]
pub use test_support::Fixture as TestFixture;
mod vectors;
#[cfg(any(test, feature = "test-support"))]
use super::policy::Root;
use little_clip_core::file_inventory::Entry;
pub use restore::{PreparedRestore, RestoreWorker, prepare_in_helper};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    cell::Cell,
    fs::{File, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{OpenOptionsExt, PermissionsExt},
        },
    },
    path::{Path, PathBuf},
};
#[cfg(test)]
pub use vectors::cosine_score;

pub use lexical::Names as LexicalNames;
// Both accelerators share a single allocation allowance and catalog epoch.
const WORKING_CACHE_BUDGET: usize = 512 * 1024 * 1024;
const LEXICAL_CACHE_BUDGET: usize = 16 * 1024 * 1024;
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CacheEpoch(u64, i64);

pub const DIMENSION: usize = crate::EMBEDDING_DIMENSION;
pub const SCHEMA_VERSION: i64 = 6;
const FILE_COLUMNS: &str = r#"id INTEGER PRIMARY KEY, device INTEGER NOT NULL, inode INTEGER NOT NULL,
 revision INTEGER NOT NULL, pipeline TEXT NOT NULL, metadata TEXT NOT NULL,
 state TEXT NOT NULL DEFAULT 'pending', coverage TEXT, error TEXT, extraction_version INTEGER NOT NULL DEFAULT 0,
 scope TEXT NOT NULL DEFAULT '', format BLOB NOT NULL DEFAULT X'', UNIQUE(device,inode,scope,format)"#;
// v3 adds guarded SVG/source-code extraction and explicit name-only budget outcomes.
pub const EXTRACTION_VERSION: i64 = 4;
const DIRECTORY_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS directories (\n root_id INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,\n path BLOB NOT NULL, generation INTEGER NOT NULL, complete INTEGER NOT NULL,\n PRIMARY KEY(root_id,path)\n);\n";
const SCHEMA: &str = r#"
CREATE TABLE roots (
 id INTEGER PRIMARY KEY, path BLOB NOT NULL UNIQUE, device INTEGER NOT NULL,
 inode INTEGER NOT NULL, generation INTEGER NOT NULL DEFAULT 0,
 available INTEGER NOT NULL DEFAULT 0, state TEXT NOT NULL DEFAULT 'checking', error TEXT
);
CREATE TABLE paths (
 path BLOB PRIMARY KEY, parent BLOB NOT NULL, file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE
);
CREATE TABLE memberships (
 root_id INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
 path BLOB NOT NULL REFERENCES paths(path) ON DELETE CASCADE,
 generation INTEGER NOT NULL, verified INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(root_id,path)
);
CREATE TABLE segments (
 id INTEGER PRIMARY KEY, file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
 modality TEXT NOT NULL, location TEXT NOT NULL, text TEXT NOT NULL,
 input_hash BLOB NOT NULL, pipeline TEXT NOT NULL, vector BLOB CHECK(vector IS NULL OR length(vector)=1536)
);
CREATE INDEX membership_paths ON memberships(path);
CREATE INDEX segment_file ON segments(file_id);
CREATE INDEX segment_input ON segments(pipeline,modality,input_hash);
PRAGMA user_version=6;
"#;
// Derived lookup indexes do not change the current record/backup format.
const QUERY_INDEXES: &str = "CREATE INDEX IF NOT EXISTS path_file_lookup ON paths(file_id,path);
    CREATE INDEX IF NOT EXISTS path_parent ON paths(parent);
    CREATE INDEX IF NOT EXISTS file_search ON files(id,pipeline,extraction_version,state,scope);
    CREATE INDEX IF NOT EXISTS segment_identity ON segments(id,file_id);
    CREATE INDEX IF NOT EXISTS segment_admission ON segments(file_id,modality);
    CREATE INDEX IF NOT EXISTS segment_vector_file ON segments(file_id) WHERE vector IS NOT NULL;
    CREATE INDEX IF NOT EXISTS membership_search ON memberships(path,root_id,verified);
    DROP INDEX IF EXISTS path_file;";
const FTS_SCHEMA: &str = r#"
CREATE VIRTUAL TABLE segment_fts USING fts5(text,content='segments',content_rowid='id',tokenize='unicode61 remove_diacritics 2 categories ''L* N* Co M*''');
CREATE TRIGGER segment_insert AFTER INSERT ON segments BEGIN
 INSERT INTO segment_fts(rowid,text) VALUES (new.id,new.text); END;
CREATE TRIGGER segment_delete AFTER DELETE ON segments BEGIN
 INSERT INTO segment_fts(segment_fts,rowid,text) VALUES ('delete',old.id,old.text); END;
"#;

/// Sole-writer SQLite index with bounded derived vector and lexical caches.
///
/// Call from the existing background coordinator. Store records are observations,
/// not authorization to read a source; callers retain guarded source validation.
/// Dropping the store releases its owner lock, allowing explicit recovery/reopen.
pub struct Store {
    pub connection: Connection,
    vectors: std::sync::Arc<std::sync::Mutex<vectors::VectorCache>>,
    lexical: std::cell::RefCell<lexical::Cache>,
    owner: IndexOwner,
    vector_count: u64,
    storage_fault: Cell<bool>,
    process_scope: uuid::Uuid,
}
#[derive(Clone, Debug)]
/// Index coverage and availability for one enrolled root.
pub struct RootState {
    pub id: i64,
    pub path: PathBuf,
    pub state: String,
    pub error: Option<String>,
    pub files: usize,
    pub vectors: usize,
    pub mount: Option<u64>,
}
impl RootState {
    pub fn covers(&self, path: &Path, mount: Option<u64>) -> bool {
        path == self.path
            || (path.starts_with(&self.path) && mount.is_some() && self.mount == mount)
    }
}
/// One extracted passage and its optional canonical embedding.
pub struct Segment {
    pub modality: String,
    pub location: String,
    pub text: String,
    pub input_hash: Vec<u8>,
    pub vector: Option<Vec<f32>>,
}
impl Store {
    /// Opens the existing private index, retaining exclusive writer ownership.
    ///
    /// # Errors
    /// Returns storage admission, owner-lock, schema or SQLite failures.
    pub fn open(folder: &Path) -> Result<Self, String> {
        Self::from_owner(IndexOwner::acquire(folder)?)
    }
    fn from_owner(owner: IndexOwner) -> Result<Self, String> {
        let connection = open_database(&owner.held().join("index.sqlite"))?;
        let mut store = Self {
            vector_count: 0,
            storage_fault: Cell::new(false),
            process_scope: uuid::Uuid::new_v4(),
            connection,
            vectors: Default::default(),
            lexical: Default::default(),
            owner,
        };
        store.vector_count =
            safety::apply_content_policy(&mut store.connection, &store.storage_fault)?;
        Ok(store)
    }
    pub fn capacity(
        &self,
        file: Option<i64>,
        segments: &[Segment],
        extra_vectors: u64,
        staging_bytes: u64,
        limits: &little_clip_core::config::FileSearchLimits,
    ) -> Result<Option<String>, String> {
        // Admission starts a new attempt and clears a previously handled failure.
        self.reset_storage_fault()?;
        let old = file
            .map(|id| file_vectors(&self.connection, id, &self.storage_fault))
            .transpose()?
            .unwrap_or(0);
        let pending = segments
            .iter()
            .filter(|segment| segment.vector.is_some())
            .count() as u64;
        let projected = self
            .vector_count
            .saturating_sub(old)
            .saturating_add(pending)
            .saturating_add(extra_vectors);
        if projected > limits.max_vectors {
            return Ok(Some(format!(
                "Index vector limit reached ({}/{}) · raise the limit in Search settings or remove indexed folders",
                self.vector_count, limits.max_vectors
            )));
        }
        let bytes = self.storage_bytes()?;
        let pending_bytes = segments.iter().fold(0u64, |sum, segment| {
            sum.saturating_add((segment.text.len() + segment.location.len() + 2048) as u64)
        });
        // Reserve space for duplicated WAL/FTS pages and the next bounded passage.
        // This is conservative soft admission, not an exact SQLite size forecast.
        let reserve = pending_bytes
            .saturating_add(staging_bytes)
            .saturating_add(extra_vectors.saturating_mul(10_240))
            .saturating_mul(8)
            .saturating_add(1_048_576);
        if bytes.saturating_add(reserve) > limits.max_mib.saturating_mul(1_048_576) {
            return Ok(Some(format!(
                "Index storage limit reached ({} MiB used; {} MiB limit, including write reserve) · raise the limit in Search settings",
                bytes.div_ceil(1_048_576),
                limits.max_mib
            )));
        }
        Ok(None)
    }
    fn storage_bytes(&self) -> Result<u64, String> {
        let held = PathBuf::from(format!(
            "/proc/self/fd/{}",
            self.owner.directory.as_raw_fd()
        ));
        let mut bytes = 0u64;
        for name in ["index.sqlite", "index.sqlite-wal", "index.sqlite-shm"] {
            match std::fs::metadata(held.join(name)) {
                Ok(metadata) => bytes = bytes.saturating_add(metadata.len()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(bytes)
    }
    pub fn reset_storage_fault(&self) -> Result<(), String> {
        self.storage_fault.set(false);
        self.connection
            .query_row("SELECT 1", [], |row| row.get::<_, i64>(0))
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        Ok(())
    }

    pub fn storage_failed(&self) -> bool {
        if self.storage_fault.get() {
            return true;
        }
        // SAFETY: the connection owns this live SQLite handle throughout the call.
        let code = unsafe { rusqlite::ffi::sqlite3_errcode(self.connection.handle()) };
        matches!(
            code,
            rusqlite::ffi::SQLITE_FULL
                | rusqlite::ffi::SQLITE_IOERR
                | rusqlite::ffi::SQLITE_READONLY
        )
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn enroll(&self, root: &Root) -> Result<i64, String> {
        root.revalidate()?;
        self.enroll_source(&crate::source::SourceRoot::from_root(root))
    }
    /// A fresh explicit enrollment can replace a disabled cached lifetime.
    /// Automatic reconciliation must keep using the original expected identity.
    pub fn enroll_new_source(&mut self, root: &crate::source::SourceRoot) -> Result<i64, String> {
        let obsolete: Option<i64> = self.connection.query_row("SELECT id FROM roots WHERE path=?1 AND state='disabled' AND (device<>?2 OR inode<>?3)", params![root.path.as_os_str().as_bytes(), root.device as i64, root.inode as i64], |row| row.get(0)).optional().map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        if let Some(id) = obsolete {
            self.remove_root(id)?;
        }
        self.enroll_source(root)
    }
    pub fn enroll_source(&self, root: &crate::source::SourceRoot) -> Result<i64, String> {
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        let changed: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM roots WHERE path=?1 AND (device<>?2 OR inode<>?3))",
                params![
                    root.path.as_os_str().as_bytes(),
                    root.device as i64,
                    root.inode as i64
                ],
                |row| row.get(0),
            )
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        if changed {
            return Err("Indexed folder identity changed; remove its enrollment before choosing a replacement".into());
        }
        tx.execute("INSERT INTO roots(path,device,inode) VALUES (?1,?2,?3) ON CONFLICT(path) DO UPDATE SET device=excluded.device,inode=excluded.inode,state='checking',available=0,error=NULL",
            params![root.path.as_os_str().as_bytes(), root.device as i64, root.inode as i64]).map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        let id = tx
            .query_row(
                "SELECT id FROM roots WHERE path=?1",
                [root.path.as_os_str().as_bytes()],
                |r| r.get(0),
            )
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        self.synchronize_source(&tx, id, root)
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        tx.commit()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        Ok(id)
    }
    pub fn root_id(&self, path: &Path) -> Result<Option<i64>, String> {
        self.connection
            .query_row(
                "SELECT id FROM roots WHERE path=?1",
                [path.as_os_str().as_bytes()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))
    }
    pub fn unavailable(&self, path: &Path, error: &str) -> Result<Option<i64>, String> {
        let id = self.root_id(path)?;
        if let Some(id) = id {
            self.break_source_continuity(id)?;
            self.connection
                .execute(
                    "UPDATE roots SET available=0,state=CASE WHEN state='disabled' THEN 'disabled' ELSE 'offline' END,error=?2 WHERE id=?1",
                    params![id, error],
                )
                .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        }
        Ok(id)
    }

    pub fn cache_epoch(&self) -> Result<CacheEpoch, String> {
        let version = self
            .connection
            .pragma_query_value(None, "data_version", |row| row.get(0))
            .map_err(|error| error.to_string())?;
        Ok(CacheEpoch(self.connection.total_changes(), version))
    }

    pub fn roots(&self) -> Result<Vec<RootState>, String> {
        // Memberships are unique per root/path. Deduplicate file IDs before
        // counting vectors so multiple compatible hardlinks count only once.
        // The partial vector index avoids reading canonical vector payloads.
        let mut query = self
            .connection
            .prepare(
                "SELECT r.id,r.path,r.state,r.error,
              (SELECT count(*) FROM memberships m WHERE m.root_id=r.id),
              (SELECT count(*) FROM
                (SELECT DISTINCT p.file_id FROM memberships m JOIN paths p ON p.path=m.path
                  JOIN files f INDEXED BY file_search ON f.id=p.file_id
                  WHERE m.root_id=r.id AND m.verified=1 AND f.state='ready') eligible
                JOIN segments s INDEXED BY segment_vector_file ON s.file_id=eligible.file_id
                WHERE s.vector IS NOT NULL)
              FROM roots r WHERE r.state<>'disabled' ORDER BY r.path",
            )
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        let rows = query
            .query_map([], |row| {
                use std::os::unix::ffi::OsStringExt;
                Ok(RootState {
                    id: row.get(0)?,
                    path: std::ffi::OsString::from_vec(row.get(1)?).into(),
                    state: row.get(2)?,
                    error: row.get(3)?,
                    files: row.get(4)?,
                    vectors: row.get(5)?,
                    mount: None,
                })
            })
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))
    }
    pub fn source_roots(&self) -> Result<Vec<crate::source::SourceRoot>, String> {
        use std::os::unix::ffi::OsStringExt;
        let mut query = self
            .connection
            .prepare("SELECT path,device,inode FROM roots WHERE available=1")
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        query
            .query_map([], |row| {
                Ok(crate::source::SourceRoot {
                    path: std::ffi::OsString::from_vec(row.get(0)?).into(),
                    device: row.get::<_, i64>(1)? as u64,
                    inode: row.get::<_, i64>(2)? as u64,
                    mount: None,
                    session: None,
                })
            })
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))
    }
    /// Identify last-known candidates only where no available membership exists.
    pub fn last_known_paths(
        &self,
        entries: &[Entry],
    ) -> Result<std::collections::HashSet<PathBuf>, String> {
        let mut statement = self.connection.prepare("SELECT EXISTS(SELECT 1 FROM memberships m JOIN roots r ON r.id=m.root_id WHERE m.path=?1 AND r.state='offline' AND r.available=0) AND NOT EXISTS(SELECT 1 FROM memberships m JOIN roots r ON r.id=m.root_id WHERE m.path=?1 AND r.available=1)").map_err(|e| e.to_string())?;
        let mut result = std::collections::HashSet::new();
        for entry in entries {
            if statement
                .query_row([entry.path.as_os_str().as_bytes()], |row| {
                    row.get::<_, bool>(0)
                })
                .map_err(|e| e.to_string())?
            {
                result.insert(entry.path.clone());
            }
        }
        Ok(result)
    }
    pub fn monitor_roots(&self) -> Result<Vec<(i64, crate::source::SourceRoot)>, String> {
        use std::os::unix::ffi::OsStringExt;
        let mut query = self
            .connection
            .prepare("SELECT id,path,device,inode FROM roots")
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        query
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    crate::source::SourceRoot {
                        path: std::ffi::OsString::from_vec(row.get(1)?).into(),
                        device: row.get::<_, i64>(2)? as u64,
                        inode: row.get::<_, i64>(3)? as u64,
                        mount: None,
                        session: None,
                    },
                ))
            })
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))
    }
    /// Configuration owns enrollment; cached observations cannot reactivate it.
    pub fn disable_unconfigured(&mut self, configured: &[PathBuf]) -> Result<(), String> {
        let roots = self.monitor_roots()?;
        if roots.is_empty() {
            return Ok(());
        }
        let tx = self
            .connection
            .transaction()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        for (id, root) in roots {
            if configured.contains(&root.path) {
                // Re-expose configured intent without trusting cached availability.
                tx.execute(
                    "UPDATE roots SET state='checking' WHERE id=?1 AND state='disabled'",
                    [id],
                )
                .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
            } else {
                tx.execute(
                    "UPDATE roots SET available=0,state='disabled' WHERE id=?1",
                    [id],
                )
                .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
                tx.execute("UPDATE memberships SET verified=0 WHERE root_id=?1", [id])
                    .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
            }
        }
        tx.commit()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))
    }
    pub fn invalidate_directory(&self, root: i64, path: Option<&Path>) -> Result<(), String> {
        self.connection.execute("UPDATE memberships SET verified=0 WHERE root_id=?1 AND (?2 IS NULL OR path IN (SELECT path FROM paths WHERE parent=?2))",
            params![root, path.map(|p| p.as_os_str().as_bytes())]).map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        self.connection.execute("UPDATE roots SET state=CASE WHEN state='disabled' THEN 'disabled' WHEN available=0 THEN 'offline' ELSE 'checking' END WHERE id=?1", [root]).map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        Ok(())
    }
    pub fn begin_delta(&mut self, root: i64) -> Result<i64, String> {
        self.connection.query_row("UPDATE roots SET generation=generation+1,state='building',error=NULL WHERE id=?1 AND available=1 RETURNING generation",
            [root], |row| row.get(0)).map_err(|error| record_storage_fault(&self.storage_fault, error))
    }
    pub fn known_directory(&self, root: i64, path: &Path) -> Result<bool, String> {
        self.connection.query_row("SELECT EXISTS(SELECT 1 FROM directories WHERE root_id=?1 AND path=?2 AND complete=1)",
            params![root, path.as_os_str().as_bytes()], |row| row.get(0)).map_err(|error| record_storage_fault(&self.storage_fault, error))
    }

    pub fn begin_scan(&mut self, root: i64) -> Result<i64, String> {
        let tx = self
            .connection
            .transaction()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        tx.execute("UPDATE roots SET generation=generation+1,state='building',available=1,error=NULL WHERE id=?1", [root]).map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        // Unverified old inventory never authorizes current semantic results.
        tx.execute("UPDATE memberships SET verified=0 WHERE root_id=?1", [root])
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        let generation = tx
            .query_row("SELECT generation FROM roots WHERE id=?1", [root], |row| {
                row.get(0)
            })
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        tx.commit()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        Ok(generation)
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn observe(
        &mut self,
        root: i64,
        generation: i64,
        entry: &Entry,
        metadata: &std::fs::Metadata,
        pipeline: &str,
    ) -> Result<(i64, bool), String> {
        use std::os::unix::fs::MetadataExt;
        self.observe_object(
            root,
            generation,
            entry,
            (metadata.dev(), metadata.ino()),
            pipeline,
        )
    }
    pub fn observe_object(
        &mut self,
        root: i64,
        generation: i64,
        entry: &Entry,
        object: (u64, u64),
        pipeline: &str,
    ) -> Result<(i64, bool), String> {
        let scope = self.source_scope_for(root)?;
        let format = source_format(entry);
        // Retry the obsolete RTF name-only outcome without invalidating other
        // formats or compatible canonical vectors.
        let rtf = little_clip_core::document::is_rtf(&entry.path);
        let json = serde_json::to_string(entry).map_err(|error| error.to_string())?;
        let tx = self
            .connection
            .transaction()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        let unchanged: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM files WHERE device=?1 AND inode=?2 AND revision=?3 AND pipeline=?4 AND extraction_version=?5 AND scope=?6 AND format=?9 AND state IN ('ready','unsupported') AND coalesce(json_extract(metadata,'$.typescript'),0)=?7 AND NOT(state='unsupported' AND coverage='name-only: extractor unavailable' AND ?8) AND NOT EXISTS(SELECT 1 FROM file_checkpoints c WHERE c.file_id=files.id) AND NOT EXISTS(SELECT 1 FROM file_retries r WHERE r.file_id=files.id))",
            params![object.0 as i64, object.1 as i64, entry.revision as i64, pipeline, EXTRACTION_VERSION, scope, entry.typescript, rtf, format],
            |row| row.get(0)).map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        tx.execute("INSERT INTO files(device,inode,revision,pipeline,metadata,extraction_version,scope,format) VALUES (?1,?2,?3,?4,?5,?6,?7,?9) ON CONFLICT(device,inode,scope,format) DO UPDATE SET revision=excluded.revision,pipeline=excluded.pipeline,metadata=excluded.metadata,extraction_version=excluded.extraction_version,state=CASE WHEN files.revision=excluded.revision AND files.pipeline=excluded.pipeline AND files.extraction_version=excluded.extraction_version AND coalesce(json_extract(files.metadata,'$.typescript'),0)=coalesce(json_extract(excluded.metadata,'$.typescript'),0) AND NOT(files.state='unsupported' AND files.coverage='name-only: extractor unavailable' AND ?8) THEN files.state ELSE 'pending' END",
            params![object.0 as i64,object.1 as i64,entry.revision as i64,pipeline,json,EXTRACTION_VERSION,scope,rtf,format]).map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        let id = tx
            .query_row(
                "SELECT id FROM files WHERE device=?1 AND inode=?2 AND scope=?3 AND format=?4",
                params![object.0 as i64, object.1 as i64, scope, format],
                |r| r.get(0),
            )
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        tx.execute("DELETE FROM file_retries WHERE file_id=?1 AND (revision<>?2 OR pipeline<>?3 OR extraction_version<>?4)",
            params![id,entry.revision as i64,pipeline,EXTRACTION_VERSION]).map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        tx.execute("DELETE FROM file_checkpoints WHERE file_id=?1 AND (revision<>?2 OR pipeline<>?3 OR extraction_version<>?4)",
            params![id,entry.revision as i64,pipeline,EXTRACTION_VERSION]).map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        tx.execute("INSERT INTO paths(path,parent,file_id) VALUES (?1,?2,?3) ON CONFLICT(path) DO UPDATE SET file_id=excluded.file_id,parent=excluded.parent",
            params![entry.path.as_os_str().as_bytes(),entry.path.parent().unwrap_or(Path::new("/")).as_os_str().as_bytes(),id]).map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        tx.execute("INSERT INTO memberships(root_id,path,generation,verified) VALUES (?1,?2,?3,1) ON CONFLICT(root_id,path) DO UPDATE SET generation=excluded.generation,verified=1", params![root,entry.path.as_os_str().as_bytes(),generation]).map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        tx.commit()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        Ok((id, !unchanged))
    }
    /// Reuse only an exact canonical input under its original model pipeline.
    /// Candidate source freshness is irrelevant: the current prepared input is
    /// checked independently, and stale vectors are never search candidates.
    pub fn reusable_vector(
        &self,
        pipeline: &str,
        modality: &str,
        hash: &[u8],
    ) -> Result<Option<Vec<f32>>, String> {
        let bytes: Option<Vec<u8>> = self.connection.query_row(
            "SELECT vector FROM segments WHERE pipeline=?1 AND modality=?2 AND input_hash=?3 AND vector IS NOT NULL LIMIT 1",
            params![pipeline, modality, hash], |row| row.get(0)
        ).optional().map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        bytes.map(|bytes| decode_vector(&bytes)).transpose()
    }

    /// Delay orphan-vector retirement until the admitted reconciliation batch
    /// drains, so a move's source directory cannot erase its destination's reuse.
    pub fn prune_unreferenced(&mut self) -> Result<(), String> {
        let tx = self
            .connection
            .transaction()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        prune(&tx, true, &self.storage_fault)?;
        tx.commit()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        self.vector_count = count_vectors(&self.connection)?;
        Ok(())
    }

    pub fn directory_complete(
        &mut self,
        root: i64,
        generation: i64,
        parent: &Path,
    ) -> Result<(), String> {
        let tx = self
            .connection
            .transaction()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        // Retirement is separate: the caller must first obtain a bounded final
        // absence observation, even after a successful directory enumeration.
        tx.execute("INSERT INTO directories(root_id,path,generation,complete) VALUES (?1,?2,?3,1) ON CONFLICT(root_id,path) DO UPDATE SET generation=excluded.generation,complete=1", params![root,parent.as_os_str().as_bytes(),generation]).map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        tx.commit()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))
    }
    pub fn unseen_children(
        &self,
        root: i64,
        generation: i64,
        parent: &Path,
    ) -> Result<Vec<PathBuf>, String> {
        use std::os::unix::ffi::OsStringExt;
        let mut query = self.connection.prepare("SELECT m.path FROM memberships m JOIN paths p ON p.path=m.path WHERE m.root_id=?1 AND m.generation<>?2 AND p.parent=?3").map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        let mut paths = query
            .query_map(
                params![root, generation, parent.as_os_str().as_bytes()],
                |row| Ok(PathBuf::from(std::ffi::OsString::from_vec(row.get(0)?))),
            )
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        let mut directories = self
            .connection
            .prepare("SELECT path FROM directories WHERE root_id=?1 AND generation<>?2")
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        for row in directories
            .query_map(params![root, generation], |row| row.get::<_, Vec<u8>>(0))
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?
        {
            let path = PathBuf::from(std::ffi::OsString::from_vec(
                row.map_err(|error| record_storage_fault(&self.storage_fault, error))?,
            ));
            if path.parent() == Some(parent) {
                paths.push(path);
            }
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
    /// Retire this root’s generated references after policy exclusion or verified absence.
    pub fn retire_path(&mut self, root: i64, path: &Path) -> Result<(), String> {
        let tx = self
            .connection
            .transaction()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        let bytes = path.as_os_str().as_bytes();
        let mut lower = bytes.to_vec();
        if lower.last() != Some(&b'/') {
            lower.push(b'/');
        }
        let mut upper = lower.clone();
        *upper.last_mut().unwrap() += 1;
        // Use the same lossless slash-prefix range as recursive retrieval.
        // Equality and descendant ranges each use the existing key indexes.
        for table in ["memberships", "directories"] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE root_id=?1 AND path=?2"),
                params![root, bytes],
            )
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
            tx.execute(
                &format!("DELETE FROM {table} WHERE root_id=?1 AND path>=?2 AND path<?3"),
                params![root, lower, upper],
            )
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        }
        tx.execute("DELETE FROM paths WHERE path=?1 AND NOT EXISTS(SELECT 1 FROM memberships m WHERE m.path=paths.path)",[bytes])
            .map_err(|error| record_storage_fault(&self.storage_fault,error))?;
        tx.execute("DELETE FROM paths WHERE path>=?1 AND path<?2 AND NOT EXISTS(SELECT 1 FROM memberships m WHERE m.path=paths.path)",params![lower,upper])
            .map_err(|error| record_storage_fault(&self.storage_fault,error))?;
        tx.commit()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))
    }
    pub fn scan_state(&self, root: i64) -> Result<String, String> {
        self.connection.query_row("SELECT CASE WHEN EXISTS(SELECT 1 FROM memberships m WHERE m.root_id=?1 AND m.verified=0) THEN 'partial' WHEN EXISTS(SELECT 1 FROM memberships m JOIN paths p ON p.path=m.path JOIN files f ON f.id=p.file_id WHERE m.root_id=?1 AND m.verified=1 AND (f.state='pending' OR (f.state='ready' AND EXISTS(SELECT 1 FROM file_checkpoints c WHERE c.file_id=f.id)))) THEN 'building' WHEN EXISTS(SELECT 1 FROM memberships m JOIN paths p ON p.path=m.path JOIN files f ON f.id=p.file_id WHERE m.root_id=?1 AND m.verified=1 AND (f.state='failed' OR EXISTS(SELECT 1 FROM file_retries r WHERE r.file_id=f.id))) THEN 'partial' ELSE 'ready' END", [root], |row| row.get(0))
            .map_err(|error| record_storage_fault(&self.storage_fault, error))
    }
    pub fn finish_scan_state(
        &self,
        root: i64,
        state: &str,
        error: Option<&str>,
    ) -> Result<(), String> {
        self.connection
            .execute(
                "UPDATE roots SET state=?2,error=?3 WHERE id=?1",
                params![root, state, error],
            )
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        Ok(())
    }
    pub fn finish_scan(&self, root: i64, error: Option<&str>) -> Result<(), String> {
        let state = if error.is_some() {
            "error".into()
        } else {
            self.scan_state(root)?
        };
        self.finish_scan_state(root, &state, error)
    }
    pub fn remove_root(&mut self, root: i64) -> Result<(), String> {
        let tx = self
            .connection
            .transaction()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        tx.execute("DELETE FROM roots WHERE id=?1", [root])
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        prune(&tx, true, &self.storage_fault)?;
        tx.commit()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        self.vector_count = count_vectors(&self.connection)?;
        Ok(())
    }
}
fn open_private(path: &Path) -> Result<File, String> {
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            little_clip_core::kernel_interface::open_content(path, false)
                .map_err(|e| e.to_string())?
        }
        Err(error) => return Err(error.to_string()),
    };
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    Ok(file)
}

fn open_database(path: &Path) -> Result<Connection, String> {
    let database = open_private(path)?;
    recovery::check_header(&database)?;
    // SQLite rejects the deliberate /proc/self/fd parent alias with OPEN_NOFOLLOW.
    // Retain the checked parent, admit/create the private file without following
    // its final component, and open an existing database only (never CREATE here).
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE;
    let connection = Connection::open_with_flags(path, flags).map_err(|e| e.to_string())?;
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|e| e.to_string())?;
    if version > SCHEMA_VERSION {
        return Err("Unsupported search index version; existing data was preserved".into());
    }
    connection
        .busy_timeout(std::time::Duration::from_secs(2))
        .map_err(|e| e.to_string())?;
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON; PRAGMA cache_size=-32768;",
        )
        .map_err(|e| e.to_string())?;
    migrate(&connection, version).map_err(|e| e.to_string())?;
    Ok(connection)
}

/// Apply every required schema step together; failure retains the prior version.
fn create_schema(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(&format!(
        "CREATE TABLE files({FILE_COLUMNS}); {SCHEMA} {DIRECTORY_SCHEMA} {} {} {} {}",
        session::SCHEMA,
        publication::SCHEMA,
        retries::SCHEMA,
        fingerprints::SCHEMA
    ))
}
fn migrate(connection: &Connection, version: i64) -> rusqlite::Result<()> {
    let foreign_keys: bool =
        connection.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    // SQLite requires foreign keys disabled *before* rebuilding a referenced
    // table. IDs and references are checked inside the same rollbackable upgrade.
    if matches!(version, 1..=5) {
        connection.pragma_update(None, "foreign_keys", false)?;
    }
    let result = migrate_steps(connection, version);
    connection.pragma_update(None, "foreign_keys", foreign_keys)?;
    result
}
fn migrate_steps(connection: &Connection, version: i64) -> rusqlite::Result<()> {
    let tx = connection.unchecked_transaction()?;
    if version == 0 {
        create_schema(&tx)?;
        tx.execute_batch(FTS_SCHEMA)?;
    }
    if version == 1 {
        tx.execute_batch("ALTER TABLE segments ADD COLUMN pipeline TEXT NOT NULL DEFAULT '';
            UPDATE segments SET pipeline=(SELECT pipeline FROM files WHERE files.id=segments.file_id)
                WHERE file_id IN (SELECT id FROM files WHERE state='ready');
            CREATE INDEX segment_input ON segments(pipeline,modality,input_hash);
            PRAGMA user_version=2;")?;
    }
    if matches!(version, 1 | 2) {
        tx.execute_batch(
            "ALTER TABLE files ADD COLUMN extraction_version INTEGER NOT NULL DEFAULT 0;
            PRAGMA user_version=3;",
        )?;
    }
    if matches!(version, 1..=3) {
        tx.execute_batch(&format!(
            "DROP TRIGGER IF EXISTS segment_insert;
            DROP TRIGGER IF EXISTS segment_delete;
            DROP TABLE IF EXISTS segment_fts;
            {FTS_SCHEMA}
            INSERT INTO segment_fts(segment_fts) VALUES ('rebuild');
            PRAGMA user_version=4;"
        ))?;
    }
    if matches!(version, 1..=5) {
        migrate_file_identity(&tx, version >= 5)?;
    }
    tx.execute_batch(&format!("{DIRECTORY_SCHEMA}{QUERY_INDEXES}"))?;
    tx.execute_batch(&format!(
        "{} {} {}",
        fingerprints::SCHEMA,
        publication::SCHEMA,
        retries::SCHEMA
    ))?;
    tx.commit()
}

fn migrate_inventory_columns(tx: &Connection) -> rusqlite::Result<()> {
    // Early catalogs may contain only root identity and membership visibility.
    // Restore the current inventory shape without inventing source availability.
    for (table, column, definition) in [
        ("roots", "generation", "INTEGER NOT NULL DEFAULT 0"),
        ("roots", "available", "INTEGER NOT NULL DEFAULT 0"),
        ("roots", "state", "TEXT NOT NULL DEFAULT 'checking'"),
        ("roots", "error", "TEXT"),
        ("memberships", "generation", "INTEGER NOT NULL DEFAULT 0"),
    ] {
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name=?2)",
            params![table, column],
            |row| row.get(0),
        )?;
        if !exists {
            tx.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN {column} {definition}"
            ))?;
        }
    }
    Ok(())
}

fn migrate_file_identity(tx: &Connection, scoped: bool) -> rusqlite::Result<()> {
    migrate_inventory_columns(tx)?;
    let scope = if scoped { "scope" } else { "''" };
    // Legacy rows can combine incompatible aliases. Keep IDs/vectors, but use
    // an empty representation sentinel and hide their ambiguous state until
    // each current path is observed under its actual decoder route.
    tx.execute_batch(&format!("CREATE TABLE files_next({FILE_COLUMNS});
        INSERT INTO files_next(id,device,inode,revision,pipeline,metadata,state,coverage,error,extraction_version,scope)
        SELECT id,device,inode,revision,pipeline,metadata,'pending',coverage,error,extraction_version,{scope} FROM files;
        DROP TABLE files;
        ALTER TABLE files_next RENAME TO files;
        PRAGMA user_version=6;"))?;
    if !scoped {
        tx.execute_batch(session::SCHEMA)?;
    }
    if tx.prepare("PRAGMA foreign_key_check")?.exists([])? {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(())
}

fn require_local_storage(directory: &File) -> Result<(), String> {
    let mut stats = std::mem::MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: directory owns a live descriptor and successful fstatfs initializes stats.
    if unsafe { libc::fstatfs(directory.as_raw_fd(), stats.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: successful fstatfs initialized the complete structure.
    local_index_filesystem(unsafe { stats.assume_init() }.f_type as u64)
}

fn local_index_filesystem(magic: u64) -> Result<(), String> {
    // Linux UAPI linux/magic.h; ZFS uses OpenZFS's ZFS_SUPER_MAGIC.
    // A network denylist cannot establish locality: FUSE may be remote,
    // and new/distributed filesystem types would silently bypass it.
    match magic {
        0x9123_683e  // Btrfs
        | 0xef53    // ext2/ext3/ext4
        | 0x5846_5342 // XFS
        | 0x2fc1_2fc1 // ZFS
        | 0xf2f5_2010 // F2FS
        | 0xca45_1a4e // bcachefs
        | 0x0102_1994 // tmpfs
        | 0x794c_7630 // overlayfs
        => Ok(()),
        _ => Err(format!(
            "The SQLite search index needs supported local storage (filesystem type {magic:#x}). \
             FUSE, network and unknown filesystems are not supported for the index; \
             choose a local XDG_DATA_HOME. Indexed source folders may still be remote."
        )),
    }
}

fn record_storage_fault(fault: &Cell<bool>, error: rusqlite::Error) -> String {
    if let rusqlite::Error::SqliteFailure(code, _) = &error
        && matches!(
            code.extended_code & 0xff,
            rusqlite::ffi::SQLITE_FULL
                | rusqlite::ffi::SQLITE_IOERR
                | rusqlite::ffi::SQLITE_READONLY
        )
    {
        fault.set(true);
    }
    error.to_string()
}

fn count_vectors(connection: &Connection) -> Result<u64, String> {
    connection
        .query_row(
            "SELECT COUNT(*) FROM segments WHERE vector IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())
}
fn file_vectors(connection: &Connection, file: i64, fault: &Cell<bool>) -> Result<u64, String> {
    connection
        .query_row(
            "SELECT COUNT(*) FROM segments WHERE file_id=?1 AND vector IS NOT NULL",
            [file],
            |row| row.get(0),
        )
        .map_err(|error| record_storage_fault(fault, error))
}

fn prune(tx: &rusqlite::Transaction<'_>, files: bool, fault: &Cell<bool>) -> Result<(), String> {
    tx.execute(
        "DELETE FROM paths WHERE NOT EXISTS(SELECT 1 FROM memberships m WHERE m.path=paths.path)",
        [],
    )
    .map_err(|error| record_storage_fault(fault, error))?;
    if !files {
        return Ok(());
    }
    tx.execute(
        "DELETE FROM files WHERE NOT EXISTS(SELECT 1 FROM paths p WHERE p.file_id=files.id)",
        [],
    )
    .map_err(|error| record_storage_fault(fault, error))?;
    Ok(())
}

/// Normalizes and encodes the existing canonical f16 vector representation.
///
/// # Errors
/// Returns an error for an invalid dimension, nonfinite values or a zero norm.
pub fn encode_vector(values: &[f32]) -> Result<Vec<u8>, String> {
    if values.len() != DIMENSION || values.iter().any(|v| !v.is_finite()) {
        return Err("Invalid embedding dimension or nonfinite value".into());
    }
    let norm = values
        .iter()
        .map(|&v| f64::from(v).powi(2))
        .sum::<f64>()
        .sqrt();
    if norm < 1e-12 {
        return Err("Zero embedding cannot be indexed".into());
    }
    Ok(values
        .iter()
        .flat_map(|&value| half::f16::from_f32((f64::from(value) / norm) as f32).to_le_bytes())
        .collect())
}
/// Decodes and normalizes a canonical persisted embedding.
///
/// # Errors
/// Returns an error for a corrupt representation or zero norm.
pub fn decode_vector(bytes: &[u8]) -> Result<Vec<f32>, String> {
    decode_vector_array(bytes).map(|values| values.to_vec())
}

pub fn decode_vector_array(bytes: &[u8]) -> Result<[f32; DIMENSION], String> {
    if bytes.len() != DIMENSION * 2 {
        return Err("Corrupt embedding length".into());
    }
    use half::slice::HalfFloatSliceExt;
    // Copy into aligned half values before the library's runtime-dispatched
    // batch conversion. SQLite BLOBs need not be aligned or host-endian.
    let halves: [half::f16; DIMENSION] = std::array::from_fn(|index| {
        half::f16::from_le_bytes([bytes[index * 2], bytes[index * 2 + 1]])
    });
    let mut values = [0.0; DIMENSION];
    halves.convert_to_f32_slice(&mut values);
    if values.iter().any(|v| !v.is_finite()) {
        return Err("Corrupt embedding value".into());
    }
    let norm = values
        .iter()
        .map(|&v| f64::from(v).powi(2))
        .sum::<f64>()
        .sqrt();
    if norm < 1e-12 {
        return Err("Corrupt zero embedding".into());
    }
    for value in &mut values {
        *value = (f64::from(*value) / norm) as f32;
    }
    Ok(values)
}

#[cfg(test)]
mod tests;
