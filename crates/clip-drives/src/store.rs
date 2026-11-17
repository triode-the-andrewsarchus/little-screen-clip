//! Transactional drive state, bounded metadata pages and disk-backed scan manifests.
use crate::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use std::{path::Path, time::Duration};

pub(crate) struct Store(pub Connection);
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(2))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS version (id INTEGER PRIMARY KEY CHECK(id=1), value INTEGER NOT NULL);
            INSERT OR IGNORE INTO version VALUES(1,1);
            CREATE TABLE IF NOT EXISTS entities(kind TEXT NOT NULL,id TEXT NOT NULL,json TEXT NOT NULL,PRIMARY KEY(kind,id));
            CREATE INDEX IF NOT EXISTS entity_kind ON entities(kind);
            CREATE TABLE IF NOT EXISTS entries(scan TEXT NOT NULL,path BLOB NOT NULL,parent BLOB NOT NULL,name BLOB NOT NULL,json TEXT NOT NULL,PRIMARY KEY(scan,path));
            CREATE INDEX IF NOT EXISTS entry_parent ON entries(scan,parent,name);
            CREATE TABLE IF NOT EXISTS conflict_paths(pair TEXT NOT NULL,path BLOB NOT NULL,id TEXT NOT NULL,PRIMARY KEY(pair,path,id));
            CREATE TABLE IF NOT EXISTS sync_blocked(pair TEXT NOT NULL,prefix BLOB NOT NULL,PRIMARY KEY(pair,prefix));
            CREATE TABLE IF NOT EXISTS sync_dirs(pair TEXT NOT NULL,path BLOB NOT NULL,PRIMARY KEY(pair,path));
            CREATE TABLE IF NOT EXISTS ram_pending(id INTEGER PRIMARY KEY AUTOINCREMENT,pair TEXT NOT NULL,path BLOB NOT NULL,UNIQUE(pair,path));
            CREATE TABLE IF NOT EXISTS baseline(pair TEXT NOT NULL,path BLOB NOT NULL,hash TEXT NOT NULL,PRIMARY KEY(pair,path));")?;
        Self::checked(db)
    }
    /// Connects to initialized state without acquiring a schema write lock.
    pub fn existing(path: &Path, read_only: bool) -> Result<Self> {
        let flags = if read_only {
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
        } else {
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
        };
        let db = Connection::open_with_flags(path, flags)?;
        db.busy_timeout(Duration::from_secs(2))?;
        if !read_only {
            db.execute_batch("PRAGMA synchronous=FULL")?;
        }
        Self::checked(db)
    }
    fn checked(db: Connection) -> Result<Self> {
        let version: u32 =
            db.query_row("SELECT value FROM version WHERE id=1", [], |r| r.get(0))?;
        // Version 2 is the same table layout with typed catalog coverage.
        // Its gate prevents older FUSE helpers from treating partial folders as empty.
        if !matches!(version, 1 | 2) {
            return Err("Unsupported drive database version".into());
        }
        Ok(Self(db))
    }
    pub fn put<T: Serialize>(&self, kind: &str, id: &str, value: &T) -> Result<()> {
        self.0.execute("INSERT INTO entities VALUES(?1,?2,?3) ON CONFLICT(kind,id) DO UPDATE SET json=excluded.json",params![kind,id,serde_json::to_string(value)?])?;
        Ok(())
    }
    pub fn get<T: DeserializeOwned>(&self, kind: &str, id: &str) -> Result<Option<T>> {
        let json: Option<String> = self
            .0
            .query_row(
                "SELECT json FROM entities WHERE kind=?1 AND id=?2",
                params![kind, id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(json.map(|j| serde_json::from_str(&j)).transpose()?)
    }
    pub fn list<T: DeserializeOwned>(
        &self,
        kind: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<T>> {
        let mut statement = self.0.prepare(
            "SELECT json FROM entities WHERE kind=?1 ORDER BY rowid DESC LIMIT ?2 OFFSET ?3",
        )?;
        let rows = statement
            .query_map(params![kind, limit.min(1000) as i64, offset as i64], |r| {
                r.get::<_, String>(0)
            })?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    /// Limits decoding before materializing a potentially corrupt metadata record.
    pub fn get_bounded<T: DeserializeOwned>(
        &self,
        kind: &str,
        id: &str,
        bytes: u64,
    ) -> Result<Option<T>> {
        let json: Option<Option<String>> = self.0.query_row(
            "SELECT CASE WHEN length(CAST(json AS BLOB))<=?3 THEN json END FROM entities WHERE kind=?1 AND id=?2",
            params![kind, id, bytes.min(i64::MAX as u64)], |row| row.get(0),
        ).optional()?;
        json.map(|json| {
            let json = json.ok_or("Metadata record exceeds its decoding budget")?;
            Ok(serde_json::from_str(&json)?)
        })
        .transpose()
    }
    pub fn remove(&self, kind: &str, id: &str) -> Result<()> {
        self.0.execute(
            "DELETE FROM entities WHERE kind=?1 AND id=?2",
            params![kind, id],
        )?;
        Ok(())
    }
    /// Visits a fixed rowid interval in bounded pages inside the caller's transaction.
    pub fn visit<T: DeserializeOwned>(
        &self,
        kind: &str,
        mut visit: impl FnMut(T) -> Result<()>,
    ) -> Result<()> {
        if self.0.is_autocommit() {
            return Err("Entity visits require a retained transaction".into());
        }
        let through: Option<i64> = self.0.query_row(
            "SELECT max(rowid) FROM entities WHERE kind=?1",
            [kind],
            |row| row.get(0),
        )?;
        let Some(through) = through else {
            return Ok(());
        };
        let mut after = 0_i64;
        loop {
            let page = {
                let mut statement = self.0.prepare("SELECT rowid,json FROM entities WHERE kind=?1 AND rowid>?2 AND rowid<=?3 ORDER BY rowid LIMIT 1000")?;
                let rows = statement.query_map(rusqlite::params![kind, after, through], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            let Some((last, _)) = page.last() else {
                return Ok(());
            };
            after = *last;
            for (_, json) in page {
                visit(serde_json::from_str(&json)?)?;
            }
        }
    }
    pub fn clear_scan(&self, scan: &str) -> Result<()> {
        self.0
            .execute("DELETE FROM entries WHERE scan=?1", [scan])?;
        Ok(())
    }
}
