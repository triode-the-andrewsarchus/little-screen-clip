//! Transactional index records. A scan's inventory is separate from content readiness.
mod maintenance;
mod recovery;
use recovery::IndexOwner;
mod fingerprints;
mod publication;
pub(super) use fingerprints::format as source_format;
pub(super) use publication::Checkpoint;
mod restore;
mod session;
mod vectors;
#[cfg(test)]
use super::policy::Root;
use crate::file_manager::filesystem::Entry;
pub(super) use restore::prepare_in_helper;
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
pub(super) use vectors::cosine_score;

pub(super) const DIMENSION: usize = 768;
pub(super) const SCHEMA_VERSION: i64 = 5;
const FILE_COLUMNS: &str = r#"id INTEGER PRIMARY KEY, device INTEGER NOT NULL, inode INTEGER NOT NULL,
 revision INTEGER NOT NULL, pipeline TEXT NOT NULL, metadata TEXT NOT NULL,
 state TEXT NOT NULL DEFAULT 'pending', coverage TEXT, error TEXT, extraction_version INTEGER NOT NULL DEFAULT 0,
 scope TEXT NOT NULL DEFAULT '', UNIQUE(device,inode,scope)"#;
// v3 adds guarded SVG/source-code extraction and explicit name-only budget outcomes.
pub(super) const EXTRACTION_VERSION: i64 = 4;
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
CREATE TABLE directories (
 root_id INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
 path BLOB NOT NULL, generation INTEGER NOT NULL, complete INTEGER NOT NULL,
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
PRAGMA user_version=5;
"#;
// Derived lookup indexes do not change the current record/backup format.
const QUERY_INDEXES: &str = "CREATE INDEX IF NOT EXISTS path_file_lookup ON paths(file_id,path);
    CREATE INDEX IF NOT EXISTS path_parent ON paths(parent);
    CREATE INDEX IF NOT EXISTS file_search ON files(id,pipeline,extraction_version,state,scope);
    CREATE INDEX IF NOT EXISTS segment_identity ON segments(id,file_id);
    CREATE INDEX IF NOT EXISTS segment_admission ON segments(file_id,modality);
    CREATE INDEX IF NOT EXISTS membership_search ON memberships(path,root_id,verified);
    DROP INDEX IF EXISTS path_file;";
const FTS_SCHEMA: &str = r#"
CREATE VIRTUAL TABLE segment_fts USING fts5(text,content='segments',content_rowid='id',tokenize='unicode61 remove_diacritics 2 categories ''L* N* Co M*''');
CREATE TRIGGER segment_insert AFTER INSERT ON segments BEGIN
 INSERT INTO segment_fts(rowid,text) VALUES (new.id,new.text); END;
CREATE TRIGGER segment_delete AFTER DELETE ON segments BEGIN
 INSERT INTO segment_fts(segment_fts,rowid,text) VALUES ('delete',old.id,old.text); END;
"#;

pub(super) struct Store {
    pub connection: Connection,
    vectors: std::sync::Arc<std::sync::Mutex<vectors::VectorCache>>,
    owner: IndexOwner,
    vector_count: u64,
    storage_fault: Cell<bool>,
    process_scope: uuid::Uuid,
}
#[derive(Clone, Debug)]
pub(super) struct RootState {
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
pub(super) struct Segment {
    pub modality: String,
    pub location: String,
    pub text: String,
    pub input_hash: Vec<u8>,
    pub vector: Option<Vec<f32>>,
}
impl Store {
    pub fn open(folder: &Path) -> Result<Self, String> {
        Self::from_owner(IndexOwner::acquire(folder)?)
    }
    fn from_owner(owner: IndexOwner) -> Result<Self, String> {
        let connection = open_database(&owner.held().join("index.sqlite"))?;
        let vector_count = count_vectors(&connection)?;
        Ok(Self {
            vector_count,
            storage_fault: Cell::new(false),
            process_scope: uuid::Uuid::new_v4(),
            connection,
            vectors: Default::default(),
            owner,
        })
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

    #[cfg(test)]
    pub fn enroll(&self, root: &Root) -> Result<i64, String> {
        root.revalidate()?;
        self.enroll_source(&super::reader::SourceRoot::from_root(root))
    }
    /// A fresh explicit enrollment can replace a disabled cached lifetime.
    /// Automatic reconciliation must keep using the original expected identity.
    pub fn enroll_new_source(&mut self, root: &super::reader::SourceRoot) -> Result<i64, String> {
        let obsolete: Option<i64> = self.connection.query_row("SELECT id FROM roots WHERE path=?1 AND state='disabled' AND (device<>?2 OR inode<>?3)", params![root.path.as_os_str().as_bytes(), root.device as i64, root.inode as i64], |row| row.get(0)).optional().map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        if let Some(id) = obsolete {
            self.remove_root(id)?;
        }
        self.enroll_source(root)
    }
    pub fn enroll_source(&self, root: &super::reader::SourceRoot) -> Result<i64, String> {
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
    pub fn unavailable(&self, path: &Path, error: &str) -> Result<Option<i64>, String> {
        let id = self
            .connection
            .query_row(
                "SELECT id FROM roots WHERE path=?1",
                [path.as_os_str().as_bytes()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
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

    pub fn roots(&self) -> Result<Vec<RootState>, String> {
        let mut query = self.connection.prepare("SELECT r.id,r.path,r.state,r.error,COUNT(DISTINCT m.path),(SELECT COUNT(*) FROM segments s JOIN files f ON f.id=s.file_id WHERE f.state='ready' AND s.vector IS NOT NULL AND EXISTS (SELECT 1 FROM paths p JOIN memberships m2 ON m2.path=p.path WHERE p.file_id=s.file_id AND m2.root_id=r.id AND m2.verified=1)) FROM roots r LEFT JOIN memberships m ON m.root_id=r.id WHERE r.state<>'disabled' GROUP BY r.id ORDER BY r.path").map_err(|error| record_storage_fault(&self.storage_fault, error))?;
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
    pub fn source_roots(&self) -> Result<Vec<super::reader::SourceRoot>, String> {
        use std::os::unix::ffi::OsStringExt;
        let mut query = self
            .connection
            .prepare("SELECT path,device,inode FROM roots WHERE available=1")
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        query
            .query_map([], |row| {
                Ok(super::reader::SourceRoot {
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
    pub fn monitor_roots(&self) -> Result<Vec<(i64, super::reader::SourceRoot)>, String> {
        use std::os::unix::ffi::OsStringExt;
        let mut query = self
            .connection
            .prepare("SELECT id,path,device,inode FROM roots")
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        query
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    super::reader::SourceRoot {
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
        self.connection.execute("UPDATE roots SET state=CASE WHEN available=0 THEN 'offline' ELSE 'checking' END WHERE id=?1", [root]).map_err(|error| record_storage_fault(&self.storage_fault, error))?;
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
    #[cfg(test)]
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
        let json = serde_json::to_string(entry).map_err(|error| error.to_string())?;
        let tx = self
            .connection
            .transaction()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        let unchanged: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM files WHERE device=?1 AND inode=?2 AND revision=?3 AND pipeline=?4 AND extraction_version=?5 AND scope=?6 AND state IN ('ready','unsupported') AND NOT EXISTS(SELECT 1 FROM file_checkpoints c WHERE c.file_id=files.id))",
            params![object.0 as i64, object.1 as i64, entry.revision as i64, pipeline, EXTRACTION_VERSION, scope],
            |row| row.get(0)).map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        tx.execute("INSERT INTO files(device,inode,revision,pipeline,metadata,extraction_version,scope) VALUES (?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(device,inode,scope) DO UPDATE SET revision=excluded.revision,pipeline=excluded.pipeline,metadata=excluded.metadata,extraction_version=excluded.extraction_version,state=CASE WHEN files.revision=excluded.revision AND files.pipeline=excluded.pipeline AND files.extraction_version=excluded.extraction_version THEN files.state ELSE 'pending' END",
            params![object.0 as i64,object.1 as i64,entry.revision as i64,pipeline,json,EXTRACTION_VERSION,scope]).map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        let id = tx
            .query_row(
                "SELECT id FROM files WHERE device=?1 AND inode=?2 AND scope=?3",
                params![object.0 as i64, object.1 as i64, scope],
                |r| r.get(0),
            )
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
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
        prune(&tx, false, &self.storage_fault)?;
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
        let mut prefix = bytes.to_vec();
        prefix.push(b'/');
        tx.execute("DELETE FROM memberships WHERE root_id=?1 AND (path=?2 OR substr(path,1,length(?3))=?3)", params![root,bytes,prefix]).map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        tx.execute("DELETE FROM directories WHERE root_id=?1 AND (path=?2 OR substr(path,1,length(?3))=?3)", params![root,bytes,prefix]).map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        prune(&tx, false, &self.storage_fault)?;
        tx.commit()
            .map_err(|error| record_storage_fault(&self.storage_fault, error))
    }
    pub fn finish_scan(&self, root: i64, error: Option<&str>) -> Result<(), String> {
        self.connection.execute("UPDATE roots SET state=CASE WHEN ?2 IS NOT NULL THEN 'error' WHEN EXISTS(SELECT 1 FROM memberships m WHERE m.root_id=?1 AND m.verified=0) THEN 'partial' WHEN EXISTS(SELECT 1 FROM memberships m JOIN paths p ON p.path=m.path JOIN files f ON f.id=p.file_id WHERE m.root_id=?1 AND m.verified=1 AND (f.state='pending' OR (f.state='ready' AND EXISTS(SELECT 1 FROM file_checkpoints c WHERE c.file_id=f.id)))) THEN 'building' WHEN EXISTS(SELECT 1 FROM memberships m JOIN paths p ON p.path=m.path JOIN files f ON f.id=p.file_id WHERE m.root_id=?1 AND m.verified=1 AND f.state='failed') THEN 'partial' ELSE 'ready' END,error=?2 WHERE id=?1", params![root,error]).map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        Ok(())
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
        "CREATE TABLE files({FILE_COLUMNS}); {SCHEMA} {} {}",
        session::SCHEMA,
        publication::SCHEMA
    ))
}
fn migrate(connection: &Connection, version: i64) -> rusqlite::Result<()> {
    let foreign_keys: bool =
        connection.pragma_query_value(None, "foreign_keys", |row| row.get(0))?;
    // SQLite requires foreign keys disabled *before* rebuilding a referenced
    // table. IDs and references are checked inside the same rollbackable upgrade.
    if matches!(version, 1..=4) {
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
    if matches!(version, 1..=4) {
        migrate_file_identity(&tx)?;
    }
    tx.execute_batch(QUERY_INDEXES)?;
    tx.execute_batch(&format!("{} {}", fingerprints::SCHEMA, publication::SCHEMA))?;
    tx.commit()
}

fn migrate_file_identity(tx: &Connection) -> rusqlite::Result<()> {
    tx.execute_batch(&format!("CREATE TABLE files_v5({FILE_COLUMNS});
        INSERT INTO files_v5(id,device,inode,revision,pipeline,metadata,state,coverage,error,extraction_version)
        SELECT id,device,inode,revision,pipeline,metadata,state,coverage,error,extraction_version FROM files;
        DROP TABLE files;
        ALTER TABLE files_v5 RENAME TO files;
        {} PRAGMA user_version=5;", session::SCHEMA))?;
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
    let magic = unsafe { stats.assume_init() }.f_type as u64;
    if matches!(
        magic,
        0x6969 | 0xFF534D42 | 0xFE534D42 | 0x01021997 | 0x5346414F | 0x73757245
    ) {
        return Err(
            "Place the SQLite search index on a local filesystem; indexed content may be remote"
                .into(),
        );
    }
    Ok(())
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

pub(super) fn encode_vector(values: &[f32]) -> Result<Vec<u8>, String> {
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
pub(super) fn decode_vector(bytes: &[u8]) -> Result<Vec<f32>, String> {
    decode_vector_array(bytes).map(|values| values.to_vec())
}

pub(super) fn decode_vector_array(bytes: &[u8]) -> Result<[f32; DIMENSION], String> {
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
mod tests {
    use super::*;
    use crate::file_manager::filesystem::read_directory;
    use std::sync::atomic::AtomicBool;

    fn observation(folder: &Path) -> Entry {
        read_directory(folder, &AtomicBool::new(false))
            .unwrap()
            .entries
            .remove(0)
    }
    fn segment() -> Segment {
        Segment {
            modality: "text".into(),
            location: "passage:0".into(),
            text: "the cat sat".into(),
            input_hash: vec![1; 32],
            vector: Some(vec![1.0; DIMENSION]),
        }
    }
    #[test]
    fn capacity_counts_replacements_preserves_old_content_and_tracks_removal() {
        use little_clip_core::config::FileSearchLimits;
        let cache = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("one.txt"), "one").unwrap();
        let root = Root::open(source.path()).unwrap();
        let mut store = Store::open(cache.path()).unwrap();
        let root_id = store.enroll(&root).unwrap();
        let generation = store.begin_scan(root_id).unwrap();
        let entry = observation(source.path());
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
            .publish(id, entry.revision, "model", &[segment()], "complete")
            .unwrap();
        let limits = FileSearchLimits {
            max_mib: 16,
            max_vectors: 1,
        };
        assert!(
            store
                .capacity(None, &[], 1, 0, &limits)
                .unwrap()
                .unwrap()
                .contains("vector limit")
        );
        assert!(
            store
                .capacity(Some(id), &[segment()], 0, 0, &limits)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .capacity(Some(id), &[segment()], 1, 0, &limits)
                .unwrap()
                .is_some()
        );
        let mut large = segment();
        large.text = "x".repeat(2 * 1024 * 1024);
        assert!(
            store
                .capacity(Some(id), &[large], 0, 0, &limits)
                .unwrap()
                .unwrap()
                .contains("storage limit")
        );
        assert_eq!(count_vectors(&store.connection).unwrap(), 1);
        store
            .publish(id, entry.revision, "model", &[segment()], "complete")
            .unwrap();
        assert_eq!(store.vector_count, 1);
        drop(store);
        let mut store = Store::open(cache.path()).unwrap();
        assert_eq!(store.vector_count, 1);
        store.remove_root(root_id).unwrap();
        assert_eq!(store.vector_count, 0);
        assert!(store.capacity(None, &[], 1, 0, &limits).unwrap().is_none());
        assert!(
            store
                .capacity(None, &[], 0, 2 * 1024 * 1024, &limits)
                .unwrap()
                .unwrap()
                .contains("storage limit")
        );
    }

    #[test]
    fn capacity_includes_wal_bytes_and_detects_sqlite_full() {
        let cache = tempfile::tempdir().unwrap();
        let mut store = Store::open(cache.path()).unwrap();
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("one.txt"), "one").unwrap();
        let root_id = store.enroll(&Root::open(source.path()).unwrap()).unwrap();
        let generation = store.begin_scan(root_id).unwrap();
        let entry = observation(source.path());
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
            .publish(id, entry.revision, "model", &[segment()], "complete")
            .unwrap();
        store.connection.execute_batch("CREATE TABLE capacity_fixture(payload BLOB); INSERT INTO capacity_fixture VALUES(zeroblob(2097152));").unwrap();
        let wal_bytes = std::fs::metadata(cache.path().join("index.sqlite-wal"))
            .unwrap()
            .len();
        assert!(wal_bytes > 2_000_000);
        assert!(store.storage_bytes().unwrap() >= wal_bytes);
        let pages: u64 = store
            .connection
            .pragma_query_value(None, "page_count", |row| row.get(0))
            .unwrap();
        store
            .connection
            .pragma_update(None, "max_page_count", pages)
            .unwrap();
        assert!(
            store
                .connection
                .execute("INSERT INTO capacity_fixture VALUES(zeroblob(2097152))", [])
                .is_err()
        );
        assert!(store.storage_failed());
        let mut large = segment();
        large.text = "large ".repeat(400_000);
        assert!(
            store
                .publish(id, entry.revision, "model", &[large], "complete")
                .is_err()
        );
        assert!(
            store.storage_failed(),
            "Transaction cleanup must preserve the disk-full failure"
        );
        let old: String = store
            .connection
            .query_row("SELECT text FROM segments WHERE file_id=?1", [id], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(old, "the cat sat");
        assert_eq!(store.vector_count, 1);
        assert!(
            store.storage_failed(),
            "Status reads must not erase the recorded failure"
        );
        store.reset_storage_fault().unwrap();
        assert!(!store.storage_failed());
    }

    #[test]
    fn no_default_enrollment_and_single_writer_ownership() {
        let folder = tempfile::tempdir().unwrap();
        let store = Store::open(folder.path()).unwrap();
        assert!(store.roots().unwrap().is_empty());
        assert!(Store::open(folder.path()).is_err());
        drop(store);
        assert!(Store::open(folder.path()).is_ok());
    }
    #[test]
    fn changed_revision_rejects_late_vectors_and_fts_is_transactional() {
        let folder = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("one.txt"), "one").unwrap();
        let root = Root::open(source.path()).unwrap();
        let mut store = Store::open(folder.path()).unwrap();
        let root_id = store.enroll(&root).unwrap();
        let generation = store.begin_scan(root_id).unwrap();
        let mut entry = observation(source.path());
        let metadata = std::fs::metadata(&entry.path).unwrap();
        let (id, pending) = store
            .observe(root_id, generation, &entry, &metadata, "pipeline1")
            .unwrap();
        assert!(pending);
        assert!(
            store
                .publish(id, entry.revision, "pipeline1", &[segment()], "complete")
                .unwrap()
        );
        let count: usize = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM segment_fts WHERE segment_fts MATCH 'cat'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        let (_, pending) = store
            .observe(root_id, generation, &entry, &metadata, "pipeline1")
            .unwrap();
        assert!(!pending);
        let old = entry.revision;
        entry.revision = old.wrapping_add(1);
        store
            .observe(root_id, generation, &entry, &metadata, "pipeline1")
            .unwrap();
        assert!(
            !store
                .publish(id, old, "pipeline1", &[segment()], "complete")
                .unwrap()
        );
        assert!(
            store
                .publish(id, entry.revision, "pipeline1", &[], "unsupported")
                .unwrap()
        );
        let count: usize = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM segment_fts WHERE segment_fts MATCH 'cat'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
    #[test]
    fn a_new_extractor_reprepares_unchanged_files_without_invalidating_exact_vectors() {
        let folder = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("one.txt"), "the cat sat").unwrap();
        let mut store = Store::open(folder.path()).unwrap();
        let root = store.enroll(&Root::open(source.path()).unwrap()).unwrap();
        let generation = store.begin_scan(root).unwrap();
        let entry = observation(source.path());
        let metadata = std::fs::metadata(&entry.path).unwrap();
        let (id, _) = store
            .observe(root, generation, &entry, &metadata, "model")
            .unwrap();
        store
            .publish(id, entry.revision, "model", &[segment()], "complete")
            .unwrap();
        store
            .connection
            .execute("UPDATE files SET extraction_version=0", [])
            .unwrap();
        let (_, pending) = store
            .observe(root, generation, &entry, &metadata, "model")
            .unwrap();
        assert!(pending);
        assert!(
            store
                .reusable_vector("model", "text", &[1; 32])
                .unwrap()
                .is_some()
        );
        store
            .publish(id, entry.revision, "model", &[segment()], "complete")
            .unwrap();
        assert!(
            !store
                .observe(root, generation, &entry, &metadata, "model")
                .unwrap()
                .1
        );
    }

    #[test]
    fn moved_input_survives_source_retirement_and_reuse_keeps_original_pipeline() {
        let folder = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let before = source.path().join("a");
        let after = source.path().join("z");
        std::fs::create_dir(&before).unwrap();
        std::fs::create_dir(&after).unwrap();
        std::fs::write(before.join("one.txt"), "the cat sat").unwrap();
        let mut store = Store::open(folder.path()).unwrap();
        let root = store.enroll(&Root::open(source.path()).unwrap()).unwrap();
        let generation = store.begin_scan(root).unwrap();
        let entry = observation(&before);
        let (id, _) = store
            .observe(
                root,
                generation,
                &entry,
                &std::fs::metadata(&entry.path).unwrap(),
                "old-pipeline",
            )
            .unwrap();
        store
            .publish(id, entry.revision, "old-pipeline", &[segment()], "complete")
            .unwrap();
        std::fs::rename(&entry.path, after.join("two.txt")).unwrap();
        // The old parent can be reconciled before the destination parent.
        store.retire_path(root, &entry.path).unwrap();
        store.directory_complete(root, generation, &before).unwrap();
        let moved = observation(&after);
        let (moved_id, pending) = store
            .observe(
                root,
                generation,
                &moved,
                &std::fs::metadata(&moved.path).unwrap(),
                "new-pipeline",
            )
            .unwrap();
        assert_eq!(id, moved_id);
        assert!(pending);
        assert!(
            store
                .reusable_vector("old-pipeline", "text", &[1; 32])
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .reusable_vector("new-pipeline", "text", &[1; 32])
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .reusable_vector("old-pipeline", "image", &[1; 32])
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .reusable_vector("old-pipeline", "text", &[2; 32])
                .unwrap()
                .is_none()
        );
        store.prune_unreferenced().unwrap();
        assert!(
            store
                .reusable_vector("old-pipeline", "text", &[1; 32])
                .unwrap()
                .is_some()
        );
        store.remove_root(root).unwrap();
        assert!(
            store
                .reusable_vector("old-pipeline", "text", &[1; 32])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn enrollment_does_not_silently_adopt_a_replacement_root() {
        let folder = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let root = Root::open(source.path()).unwrap();
        let mut store = Store::open(folder.path()).unwrap();
        let id = store.enroll(&root).unwrap();
        store.begin_scan(id).unwrap();
        let replacement = super::super::reader::SourceRoot {
            path: root.path.clone(),
            device: root.device,
            inode: root.inode.wrapping_add(1),
            mount: Some(root.mount),
            session: None,
        };
        assert!(store.enroll_source(&replacement).is_err());
        assert!(store.enroll_new_source(&replacement).is_err());
        assert_eq!(store.roots().unwrap().len(), 1);
        assert_eq!(store.source_roots().unwrap().len(), 1);
        assert_eq!(
            store.unavailable(&root.path, "Root replaced").unwrap(),
            Some(id)
        );
        assert_eq!(store.roots().unwrap()[0].state, "offline");
        assert!(store.source_roots().unwrap().is_empty());
        assert!(store.enroll(&root).is_ok());
        store.disable_unconfigured(&[]).unwrap();
        assert!(store.enroll_source(&replacement).is_err());
        store.enroll_new_source(&replacement).unwrap();
        let inode: i64 = store
            .connection
            .query_row("SELECT inode FROM roots", [], |row| row.get(0))
            .unwrap();
        assert_eq!(inode as u64, replacement.inode);
    }

    #[test]
    fn v1_migration_preserves_vectors_but_does_not_guess_pending_pipeline() {
        let folder = tempfile::tempdir().unwrap();
        let connection = Connection::open(folder.path().join("index.sqlite")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE roots(id INTEGER PRIMARY KEY,path BLOB,device INTEGER,inode INTEGER);
            CREATE TABLE files(id INTEGER PRIMARY KEY,device INTEGER,inode INTEGER,revision INTEGER DEFAULT 0,pipeline TEXT,metadata TEXT DEFAULT '{}',state TEXT,coverage TEXT,error TEXT);
            CREATE TABLE paths(path BLOB PRIMARY KEY,parent BLOB,file_id INTEGER); CREATE TABLE memberships(root_id INTEGER,path BLOB,verified INTEGER);
            CREATE TABLE segments(id INTEGER PRIMARY KEY,file_id INTEGER,modality TEXT,text TEXT DEFAULT '',input_hash BLOB,vector BLOB);
            INSERT INTO files(id,device,inode,pipeline,state) VALUES(1,1,1,'original','ready'),(2,1,2,'changed','pending');
            PRAGMA user_version=1;",
            )
            .unwrap();
        for id in [1, 2] {
            connection
                .execute(
                    "INSERT INTO segments(file_id,modality,text,input_hash,vector) VALUES(?1,'text','persisted café text',?2,?3)",
                    params![
                        id,
                        vec![id as u8; 32],
                        encode_vector(&vec![1.0; DIMENSION]).unwrap()
                    ],
                )
                .unwrap();
        }
        drop(connection);
        let store = Store::open(folder.path()).unwrap();
        assert_eq!(
            store
                .connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM segment_fts WHERE segment_fts MATCH 'cafe'",
                    [],
                    |row| row.get::<_, usize>(0)
                )
                .unwrap(),
            2
        );
        assert!(
            store
                .reusable_vector("original", "text", &[1; 32])
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .reusable_vector("changed", "text", &[2; 32])
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store
                .connection
                .query_row("SELECT COUNT(*) FROM segments", [], |row| row
                    .get::<_, usize>(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn incomplete_scans_keep_inventory_and_hard_link_memberships_are_independent() {
        let folder = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let a = source.path().join("a");
        let b = source.path().join("b");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        std::fs::write(a.join("one.txt"), "one").unwrap();
        std::fs::hard_link(a.join("one.txt"), b.join("two.txt")).unwrap();
        let mut store = Store::open(folder.path()).unwrap();
        let roots: Vec<_> = [&a, &b]
            .iter()
            .map(|path| store.enroll(&Root::open(path).unwrap()).unwrap())
            .collect();
        for (path, id) in [&a, &b].iter().zip(&roots) {
            let generation = store.begin_scan(*id).unwrap();
            let entry = observation(path);
            let metadata = std::fs::metadata(&entry.path).unwrap();
            store
                .observe(*id, generation, &entry, &metadata, "pipeline")
                .unwrap();
        }
        assert_eq!(
            store
                .connection
                .query_row("SELECT COUNT(*) FROM files", [], |r| r.get::<_, usize>(0))
                .unwrap(),
            1
        );
        store.begin_scan(roots[0]).unwrap();
        store
            .finish_scan(roots[0], Some("directory unavailable"))
            .unwrap();
        assert_eq!(store.roots().unwrap()[0].files, 1);
        store.remove_root(roots[0]).unwrap();
        assert_eq!(store.roots().unwrap()[0].files, 1);
        assert_eq!(
            store
                .connection
                .query_row("SELECT COUNT(*) FROM files", [], |r| r.get::<_, usize>(0))
                .unwrap(),
            1
        );
    }
    #[test]
    fn delta_invalidation_preserves_other_directories_and_pending_coverage_stays_partial() {
        let folder = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let mut store = Store::open(folder.path()).unwrap();
        let root = store.enroll(&Root::open(source.path()).unwrap()).unwrap();
        let generation = store.begin_scan(root).unwrap();
        for name in ["a", "b"] {
            let directory = source.path().join(name);
            std::fs::create_dir(&directory).unwrap();
            std::fs::write(directory.join("one.txt"), "text").unwrap();
            let entry = observation(&directory);
            let (id, _) = store
                .observe(
                    root,
                    generation,
                    &entry,
                    &std::fs::metadata(&entry.path).unwrap(),
                    "pipeline",
                )
                .unwrap();
            store
                .publish(id, entry.revision, "pipeline", &[segment()], "complete")
                .unwrap();
            store
                .directory_complete(root, generation, &directory)
                .unwrap();
        }
        store
            .invalidate_directory(root, Some(&source.path().join("a")))
            .unwrap();
        let next = store.begin_delta(root).unwrap();
        assert!(next > generation);
        let verified: usize = store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM memberships WHERE verified=1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(verified, 1);
        store.finish_scan(root, None).unwrap();
        assert_eq!(store.roots().unwrap()[0].state, "partial");
        assert!(
            store
                .known_directory(root, &source.path().join("b"))
                .unwrap()
        );
    }

    #[test]
    fn only_completed_directory_membership_can_retire_an_unseen_path() {
        let folder = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("one.txt"), "one").unwrap();
        let mut store = Store::open(folder.path()).unwrap();
        let id = store.enroll(&Root::open(source.path()).unwrap()).unwrap();
        let generation = store.begin_scan(id).unwrap();
        let entry = observation(source.path());
        store
            .observe(
                id,
                generation,
                &entry,
                &std::fs::metadata(&entry.path).unwrap(),
                "p",
            )
            .unwrap();
        let generation = store.begin_scan(id).unwrap();
        assert_eq!(store.roots().unwrap()[0].files, 1);
        std::fs::remove_file(&entry.path).unwrap();
        store
            .directory_complete(id, generation, source.path())
            .unwrap();
        assert_eq!(
            store.roots().unwrap()[0].files,
            1,
            "Enumeration alone cannot prove absence"
        );
        assert_eq!(
            store
                .unseen_children(id, generation, source.path())
                .unwrap(),
            [entry.path.clone()]
        );
        store.retire_path(id, &entry.path).unwrap();
        store.finish_scan(id, None).unwrap();
        let root = &store.roots().unwrap()[0];
        assert_eq!(root.files, 0);
        assert_eq!(root.state, "ready");
    }
    #[test]
    fn batched_half_decoding_matches_scalar_normalization() {
        for chunk in (0..=u16::MAX)
            .filter(|bits| half::f16::from_bits(*bits).is_finite())
            .collect::<Vec<_>>()
            .chunks(DIMENSION)
        {
            let mut halves = chunk
                .iter()
                .map(|&bits| half::f16::from_bits(bits))
                .collect::<Vec<_>>();
            halves.resize(DIMENSION, half::f16::ONE);
            let bytes = halves
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect::<Vec<_>>();
            let mut reference = halves
                .iter()
                .map(|value| value.to_f32())
                .collect::<Vec<_>>();
            let norm = reference
                .iter()
                .map(|&value| f64::from(value).powi(2))
                .sum::<f64>()
                .sqrt();
            for value in &mut reference {
                *value = (f64::from(*value) / norm) as f32;
            }
            let actual = decode_vector(&bytes).unwrap();
            assert!(
                actual
                    .iter()
                    .zip(reference)
                    .all(|(a, b)| a.to_bits() == b.to_bits())
            );
        }
        for value in [
            half::f16::NAN,
            half::f16::INFINITY,
            half::f16::NEG_INFINITY,
            half::f16::ZERO,
        ] {
            assert!(decode_vector(&value.to_le_bytes().repeat(DIMENSION)).is_err());
        }
    }

    #[test]
    fn reopening_current_schema_recreates_derived_indexes_without_rebuilding_records() {
        let folder = tempfile::tempdir().unwrap();
        let store = Store::open(folder.path()).unwrap();
        store.connection.execute_batch("DROP INDEX path_file_lookup; DROP INDEX path_parent; DROP INDEX file_search; DROP INDEX segment_identity; DROP INDEX segment_admission; DROP INDEX membership_search;").unwrap();
        drop(store);
        let reopened = Store::open(folder.path()).unwrap();
        let count: usize = reopened.connection.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='index' AND name IN ('path_file_lookup','path_parent','file_search','segment_identity','segment_admission','membership_search')",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(count, 6);
        assert_eq!(
            reopened
                .connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            SCHEMA_VERSION
        );
    }

    #[test]
    fn full_source_reuse_requires_exact_digest_format_pipeline_and_complete_publication() {
        let mut fixture = session::tests::Fixture::new();
        let store = &mut fixture.store;
        let entry = &fixture.entry;
        let id = fixture.file;
        let root_id = store.enroll(&fixture.root).unwrap();
        let generation = store.begin_scan(root_id).unwrap();
        let digest = [7; 32];
        store
            .remember_source(id, entry.revision.wrapping_add(1), "model", entry, &digest)
            .unwrap();
        assert!(
            store
                .reusable_source(entry, "model", &digest)
                .unwrap()
                .is_none()
        );
        store
            .remember_source(id, entry.revision, "model", entry, &digest)
            .unwrap();
        let (reused, coverage) = store
            .reusable_source(entry, "model", &digest)
            .unwrap()
            .unwrap();
        assert_eq!(coverage, "complete");
        assert_eq!(reused.len(), 1);
        assert_eq!(reused[0].text, "original content");
        assert_eq!(reused[0].input_hash, segment().input_hash);
        assert_eq!(
            encode_vector(reused[0].vector.as_ref().unwrap()).unwrap(),
            encode_vector(segment().vector.as_ref().unwrap()).unwrap()
        );
        assert!(
            store
                .reusable_source(entry, "different-model", &digest)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .reusable_source(entry, "model", &[8; 32])
                .unwrap()
                .is_none()
        );
        let mut different_format = entry.clone();
        different_format.path.set_extension("pdf");
        assert!(
            store
                .reusable_source(&different_format, "model", &digest)
                .unwrap()
                .is_none()
        );
        // A revision change keeps the old extraction eligible only by its full
        // digest; it is never an unchanged-metadata authorization.
        let mut changed = entry.clone();
        changed.revision = changed.revision.wrapping_add(1);
        assert!(
            store
                .observe(
                    root_id,
                    generation,
                    &changed,
                    &std::fs::metadata(&entry.path).unwrap(),
                    "model"
                )
                .unwrap()
                .1
        );
        assert!(
            store
                .reusable_source(&changed, "model", &digest)
                .unwrap()
                .is_some()
        );
        store
            .publish(
                id,
                changed.revision,
                "model",
                &[segment()],
                "partial: interrupted",
            )
            .unwrap();
        store
            .remember_source(id, changed.revision, "model", &changed, &digest)
            .unwrap();
        assert!(
            store
                .reusable_source(&changed, "model", &digest)
                .unwrap()
                .is_none()
        );
        store
            .publish(id, changed.revision, "model", &[segment()], "complete")
            .unwrap();
        store
            .remember_source(id, changed.revision, "model", &changed, &digest)
            .unwrap();
        store
            .connection
            .execute("DELETE FROM files WHERE id=?1", [id])
            .unwrap();
        assert_eq!(
            store
                .connection
                .query_row("SELECT COUNT(*) FROM source_digests", [], |row| row
                    .get::<_, usize>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn compact_vectors_are_normalized_and_invalid_payloads_cannot_publish() {
        let encoded = encode_vector(&vec![0.1; DIMENSION]).unwrap();
        assert_eq!(encoded.len(), 1536);
        let values = decode_vector(&encoded).unwrap();
        assert!((values.iter().map(|v| v * v).sum::<f32>() - 1.0).abs() < 1e-5);
        assert!(encode_vector(&vec![f32::NAN; DIMENSION]).is_err());
        assert!(encode_vector(&vec![0.0; DIMENSION]).is_err());
        assert!(decode_vector(&encoded[..1500]).is_err());
    }
}
#[test]
fn failed_schema_upgrade_rolls_back_every_prior_step() {
    let connection = Connection::open_in_memory().unwrap();
    // A malformed legacy schema lets the pipeline step start, then rejects
    // the next step. None of that partially upgraded schema may survive.
    connection.execute_batch("CREATE TABLE files(id INTEGER PRIMARY KEY,pipeline TEXT,state TEXT,extraction_version INTEGER); CREATE TABLE segments(id INTEGER PRIMARY KEY,file_id INTEGER,modality TEXT,input_hash BLOB); PRAGMA user_version=1;").unwrap();
    assert!(migrate(&connection, 1).is_err());
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    let columns = connection
        .prepare("SELECT name FROM pragma_table_info('segments')")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(!columns.iter().any(|name| name == "pipeline"));
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='segment_input'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}
