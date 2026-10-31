//! Bounded filename hints for lexical dominance; SQL still decides row eligibility.
use super::{CacheEpoch, Store};
use std::{
    collections::HashMap,
    os::unix::ffi::OsStrExt,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

pub(super) const BUDGET: usize = super::LEXICAL_CACHE_BUDGET;
const OVERHEAD: usize = 64 * 1024;
pub(in crate::file_manager::search) type Names = Arc<HashMap<i64, Box<str>>>;

#[derive(Default)]
pub(super) struct Cache {
    epoch: Option<CacheEpoch>,
    names: Names,
    bytes: usize,
    pub(super) used: Option<Instant>,
}
impl Cache {
    pub(super) fn release(&mut self) {
        *self = Self::default();
    }
}

impl Store {
    pub(in crate::file_manager::search) fn prepare_lexical_names(
        &self,
        cancel: &AtomicBool,
    ) -> Result<(CacheEpoch, Names), String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Search cancelled".into());
        }
        let epoch = self.cache_epoch()?;
        let mut cache = self.lexical.borrow_mut();
        if cache.epoch != Some(epoch) {
            // Release the previous allocation before building its replacement.
            cache.release();
            let (names, bytes) =
                build(&self.connection, BUDGET, cancel).map_err(|error| error.to_string())?;
            cache.names = Arc::new(names);
            cache.bytes = bytes;
            cache.epoch = Some(epoch);
        }
        cache.used = Some(Instant::now());
        Ok((epoch, cache.names.clone()))
    }
    pub(in crate::file_manager::search) fn finish_lexical_names(
        &self,
        epoch: CacheEpoch,
    ) -> Result<(), String> {
        if self.cache_epoch()? != epoch {
            self.lexical.borrow_mut().release();
            return Err("Search index changed during lexical retrieval; retry the query".into());
        }
        Ok(())
    }
}

fn build(
    connection: &rusqlite::Connection,
    budget: usize,
    cancel: &AtomicBool,
) -> rusqlite::Result<(HashMap<i64, Box<str>>, usize)> {
    let count: usize =
        connection.query_row("SELECT COUNT(*) FROM segments", [], |row| row.get(0))?;
    let mut names = HashMap::new();
    // Conservative bucket/control/allocator allowance. Preallocate once; never
    // grow a live map, and stop before allocating a path beyond the budget.
    let limit = count.min(budget.saturating_sub(OVERHEAD) / 96);
    if limit == 0 || names.try_reserve(limit).is_err() {
        return Ok((names, 0));
    }
    let mut bytes = OVERHEAD + names.capacity() * 40;
    if bytes > budget {
        return Ok((HashMap::new(), 0));
    }
    let mut statement = connection.prepare("SELECT s.id,p.path FROM segments s JOIN paths p ON p.file_id=s.file_id WHERE NOT EXISTS(SELECT 1 FROM paths other WHERE other.file_id=p.file_id AND other.path<>p.path) ORDER BY s.id")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        if cancel.load(Ordering::Relaxed) {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_INTERRUPT),
                Some("Search cancelled".into()),
            ));
        }
        let encoded = row.get_ref(1)?.as_blob()?;
        let path = Path::new(std::ffi::OsStr::from_bytes(encoded));
        let basename = path.file_name().unwrap_or_default();
        // Bound escaped-display and boxing temporaries within OVERHEAD even
        // for malformed catalog paths; ordinary filesystem names fit easily.
        if basename.as_encoded_bytes().len() > 1024 {
            continue;
        }
        let name = crate::file_manager::filesystem::display_path_cow(Path::new(basename));
        let needed = name.len().saturating_add(32);
        if names.len() == limit || needed > budget.saturating_sub(bytes) {
            break;
        }
        names.insert(row.get(0)?, name.into_owned().into_boxed_str());
        bytes += needed;
    }
    Ok((names, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{Connection, params};

    #[test]
    fn partial_hints_preserve_lossless_singletons_and_skip_every_alias() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE segments(id INTEGER PRIMARY KEY,file_id INTEGER,vector BLOB); CREATE TABLE paths(file_id INTEGER,path BLOB);
            INSERT INTO segments VALUES(1,1,NULL),(2,2,NULL),(3,2,NULL),(4,3,NULL);").unwrap();
        for (id, path) in [
            (1, b"/sample/invalid_\xff.txt".as_slice()),
            (2, b"/sample/alias_a"),
            (2, b"/sample/alias_b"),
            (3, b"/sample/last"),
        ] {
            connection
                .execute("INSERT INTO paths VALUES(?1,?2)", params![id, path])
                .unwrap();
        }
        connection
            .execute("INSERT INTO segments VALUES(5,4,NULL)", [])
            .unwrap();
        let oversized = format!("/sample/{}", "x".repeat(2048));
        connection
            .execute("INSERT INTO paths VALUES(4,?1)", [oversized.as_bytes()])
            .unwrap();
        let cancel = AtomicBool::new(false);
        let (names, bytes) = build(&connection, BUDGET, &cancel).unwrap();
        assert_eq!(names.len(), 2);
        assert_eq!(
            names[&1].as_ref(),
            crate::file_manager::filesystem::display_path_cow(Path::new(
                std::ffi::OsStr::from_bytes(b"invalid_\xff.txt")
            ))
            .as_ref()
        );
        assert!(!names.contains_key(&2));
        assert!(!names.contains_key(&3));
        assert!(bytes <= BUDGET);
        let (empty, bytes) = build(&connection, OVERHEAD, &cancel).unwrap();
        assert!(empty.is_empty());
        assert_eq!(bytes, 0);
        let (partial, bytes) = build(&connection, OVERHEAD + 200, &cancel).unwrap();
        assert_eq!(partial.len(), 1);
        assert!(partial.contains_key(&1));
        assert!(bytes <= OVERHEAD + 200);
        cancel.store(true, Ordering::Relaxed);
        assert!(build(&connection, BUDGET, &cancel).is_err());
    }

    #[test]
    fn local_external_generations_cancellation_and_idle_release_fence_hints() {
        let folder = tempfile::tempdir().unwrap();
        let store = Store::open(folder.path()).unwrap();
        let cancel = AtomicBool::new(false);
        let (epoch, names) = store.prepare_lexical_names(&cancel).unwrap();
        let (_, reused) = store.prepare_lexical_names(&cancel).unwrap();
        assert!(Arc::ptr_eq(&names, &reused));
        drop((names, reused));
        store.connection.execute_batch("CREATE TABLE hint_generation(value INTEGER); INSERT INTO hint_generation VALUES(1)").unwrap();
        assert!(store.finish_lexical_names(epoch).is_err());
        let (epoch, names) = store.prepare_lexical_names(&cancel).unwrap();
        drop(names);
        let external = Connection::open(folder.path().join("index.sqlite")).unwrap();
        external
            .execute("UPDATE hint_generation SET value=2", [])
            .unwrap();
        assert!(store.finish_lexical_names(epoch).is_err());
        store.prepare_lexical_names(&cancel).unwrap();
        cancel.store(true, Ordering::Relaxed);
        assert!(store.prepare_lexical_names(&cancel).is_err());
        store.release_idle_vectors(std::time::Duration::ZERO);
        let cache = store.lexical.borrow();
        assert!(cache.epoch.is_none());
        assert!(cache.names.is_empty());
        assert_eq!(cache.bytes, 0);
        assert!(cache.used.is_none());
    }
}
