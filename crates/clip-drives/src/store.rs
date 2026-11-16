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
            CREATE TABLE IF NOT EXISTS entries(scan TEXT NOT NULL,path BLOB NOT NULL,parent BLOB NOT NULL,name BLOB NOT NULL,json TEXT NOT NULL,PRIMARY KEY(scan,path));
            CREATE INDEX IF NOT EXISTS entry_parent ON entries(scan,parent,name);
            CREATE TABLE IF NOT EXISTS baseline(pair TEXT NOT NULL,path BLOB NOT NULL,hash TEXT NOT NULL,PRIMARY KEY(pair,path));")?;
        let version: u32 =
            db.query_row("SELECT value FROM version WHERE id=1", [], |r| r.get(0))?;
        if version != 1 {
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
    pub fn remove(&self, kind: &str, id: &str) -> Result<()> {
        self.0.execute(
            "DELETE FROM entities WHERE kind=?1 AND id=?2",
            params![kind, id],
        )?;
        Ok(())
    }
    pub fn clear_scan(&self, scan: &str) -> Result<()> {
        self.0
            .execute("DELETE FROM entries WHERE scan=?1", [scan])?;
        Ok(())
    }
}
