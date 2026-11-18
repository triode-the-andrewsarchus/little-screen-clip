//! Transactional drive state, bounded metadata pages and disk-backed scan manifests.
use crate::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use std::{path::Path, time::Duration};

// Malformed headers are candidates too: consumers must disclose uncertainty,
// while unrelated valid unmounted history stays outside this small index.
const VIEW_FILTER: &str = "kind IN ('catalog','backup') AND CASE WHEN json_valid(json) THEN coalesce(json_type(json,'$.mount') <> 'null',0) ELSE 1 END";

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
        db.execute_batch(&format!("CREATE INDEX IF NOT EXISTS entity_mounted_headers ON entities(kind,id) WHERE {VIEW_FILTER}"))?;
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
        // Version 2 gates typed catalog coverage. Registry version 3 additionally
        // gates image-control journals, so older daemons cannot ignore an unknown
        // mount outcome and relax its fail-closed indexing policy.
        if !matches!(version, 1..=3) {
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
    /// Bounded key pages remain stable when completed entities are removed.
    pub fn ids(&self, kind: &str, after: &str, limit: usize) -> Result<Vec<String>> {
        self.id_page(kind, after, limit, "")
    }
    /// Pages mounted or uncertain catalog/backup headers through the partial index.
    pub fn mounted_ids(&self, kind: &str, after: &str, limit: usize) -> Result<Vec<String>> {
        self.id_page(kind, after, limit, &format!(" AND {VIEW_FILTER}"))
    }
    fn id_page(&self, kind: &str, after: &str, limit: usize, filter: &str) -> Result<Vec<String>> {
        let query = format!(
            "SELECT CASE WHEN length(CAST(id AS BLOB))<=128 THEN id END FROM entities WHERE kind=?1 AND id>?2{filter} ORDER BY id LIMIT ?3"
        );
        let mut statement = self.0.prepare(&query)?;
        let rows = statement.query_map(params![kind, after, limit.min(1000) as i64], |row| {
            row.get::<_, Option<String>>(0)
        })?;
        rows.map(|row| Ok(row?.ok_or("Entity identifier exceeds its decoding budget")?))
            .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mounted_header_index_migrates_legacy_state_and_tracks_commits() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.sqlite");
        let db = Store::open(&path).unwrap();
        db.0.execute_batch("DROP INDEX entity_mounted_headers")
            .unwrap();
        for index in 0..2100 {
            db.put(
                "catalog",
                &format!("history-{index:04}"),
                &serde_json::json!({"mount": null}),
            )
            .unwrap();
        }
        db.put(
            "catalog",
            "legacy",
            &serde_json::json!({"mount": "/owned/view"}),
        )
        .unwrap();
        drop(db);
        let db = Store::open(&path).unwrap();
        assert_eq!(db.mounted_ids("catalog", "", 128).unwrap(), ["legacy"]);
        let query = format!(
            "EXPLAIN QUERY PLAN SELECT id FROM entities WHERE kind=?1 AND id>?2 AND {VIEW_FILTER} ORDER BY id LIMIT 128"
        );
        let plan: String =
            db.0.query_row(&query, ["catalog", ""], |row| row.get(3))
                .unwrap();
        assert!(plan.contains("entity_mounted_headers"), "{plan}");
        {
            let transaction = db.0.unchecked_transaction().unwrap();
            db.put("catalog", "legacy", &serde_json::json!({"mount": null}))
                .unwrap();
            assert!(db.mounted_ids("catalog", "", 128).unwrap().is_empty());
            transaction.rollback().unwrap();
        }
        assert_eq!(db.mounted_ids("catalog", "", 128).unwrap(), ["legacy"]);
        db.0.execute(
            "UPDATE entities SET json='broken json' WHERE kind='catalog' AND id='legacy'",
            [],
        )
        .unwrap();
        assert_eq!(db.mounted_ids("catalog", "", 128).unwrap(), ["legacy"]);
        db.remove("catalog", "legacy").unwrap();
        assert!(db.mounted_ids("catalog", "", 128).unwrap().is_empty());
        db.put(
            "backup",
            &"x".repeat(129),
            &serde_json::json!({"mount": "/owned/image"}),
        )
        .unwrap();
        assert!(
            db.mounted_ids("backup", "", 128)
                .unwrap_err()
                .to_string()
                .contains("decoding budget")
        );
    }

    #[test]
    fn entity_key_pages_survive_removals_without_skipping_and_reject_oversized_keys() {
        let temp = tempfile::tempdir().unwrap();
        let db = Store::open(&temp.path().join("state.sqlite")).unwrap();
        for index in 0..257 {
            let id = format!("key-{index:04}");
            db.put("pending", &id, &()).unwrap();
            db.put("unrelated", &id, &()).unwrap();
        }
        let mut after = String::new();
        let mut removed = 0;
        loop {
            let page = db.ids("pending", &after, 128).unwrap();
            let Some(last) = page.last() else {
                break;
            };
            after = last.clone();
            for id in page {
                db.remove("pending", &id).unwrap();
                removed += 1;
            }
        }
        assert_eq!(removed, 257);
        assert_eq!(db.ids("unrelated", "", 1000).unwrap().len(), 257);
        let oversized = "x".repeat(129);
        db.put("pending", &oversized, &()).unwrap();
        assert!(
            db.ids("pending", "", 128)
                .unwrap_err()
                .to_string()
                .contains("decoding budget")
        );
        assert!(db.get::<()>("pending", &oversized).unwrap().is_some());
    }
}
