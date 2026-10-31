//! Persisted continuity gates and source-scoped file identity.
use super::{Store, record_storage_fault};
use crate::file_manager::search::reader::SourceRoot;
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};

pub(super) const SCHEMA: &str = "CREATE TABLE source_sessions(
    root_id INTEGER PRIMARY KEY REFERENCES roots(id) ON DELETE CASCADE,
    observation TEXT CHECK(observation IS NULL OR length(observation)<=512),
    scope TEXT NOT NULL, continuity INTEGER NOT NULL DEFAULT 0 CHECK(continuity IN (0,1))
);";

impl Store {
    pub(super) fn synchronize_source(
        &self,
        tx: &Connection,
        id: i64,
        source: &SourceRoot,
    ) -> rusqlite::Result<()> {
        let observation = source
            .session
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let scope = self.source_scope(source, observation.as_deref());
        let reusable: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM source_sessions WHERE root_id=?1 AND observation=?2 AND scope=?3 AND continuity=1)",
            params![id, observation, scope], |row| row.get(0),
        )?;
        if !reusable {
            // Keep canonical vectors for verified-input reuse, but never reuse
            // source observations across an unknown/replaced source lifetime.
            tx.execute("UPDATE files SET state='pending' WHERE id IN (SELECT p.file_id FROM paths p JOIN memberships m ON m.path=p.path WHERE m.root_id=?1)", [id])?;
            tx.execute("UPDATE memberships SET verified=0 WHERE root_id=?1", [id])?;
            tx.execute("UPDATE directories SET complete=0 WHERE root_id=?1", [id])?;
        }
        tx.execute("INSERT INTO source_sessions(root_id,observation,scope,continuity) VALUES(?1,?2,?3,1) ON CONFLICT(root_id) DO UPDATE SET observation=excluded.observation,scope=excluded.scope,continuity=1",
            params![id, observation, scope])?;
        Ok(())
    }
    pub(super) fn source_scope(&self, source: &SourceRoot, observation: Option<&str>) -> String {
        let Some(session) = source.session.as_ref().filter(|session| session.unique()) else {
            // No unique mount observation: even two unknown observations cannot
            // establish continuity. Re-extraction still reuses matching inputs.
            return uuid::Uuid::new_v4().to_string();
        };
        let mut digest = Sha256::new();
        digest.update(observation.unwrap_or_default().as_bytes());
        if !session.persistent() {
            digest.update(self.process_scope.as_bytes());
        }
        format!("{:x}", digest.finalize())
    }
    pub(super) fn source_scope_for(&self, root: i64) -> Result<String, String> {
        self.connection
            .query_row(
                "SELECT scope FROM source_sessions WHERE root_id=?1",
                [root],
                |row| row.get(0),
            )
            .map_err(|error| record_storage_fault(&self.storage_fault, error))
    }
    pub(super) fn break_source_continuity(&self, root: i64) -> Result<(), String> {
        // Preserve last-known rows while offline. Reconnection will make them
        // pending atomically before exposing an available root again.
        self.connection
            .execute(
                "UPDATE source_sessions SET continuity=0 WHERE root_id=?1",
                [root],
            )
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::file_manager::{
        filesystem::{self, Entry},
        search::{
            policy::Root,
            store::{DIMENSION, Segment},
        },
    };
    use std::sync::atomic::AtomicBool;

    pub(crate) struct Fixture {
        pub(crate) store: Store,
        pub(crate) root: Root,
        pub(crate) entry: Entry,
        pub(crate) file: i64,
        cache: tempfile::TempDir,
        _source: tempfile::TempDir,
    }
    impl Fixture {
        pub(crate) fn new() -> Self {
            let cache = tempfile::tempdir().unwrap();
            let source = tempfile::tempdir().unwrap();
            std::fs::write(source.path().join("note.txt"), "original content").unwrap();
            let root = Root::open(source.path()).unwrap();
            let entry = filesystem::read_directory(source.path(), &AtomicBool::new(false))
                .unwrap()
                .entries
                .remove(0);
            let store = Store::open(cache.path()).unwrap();
            let mut fixture = Self {
                store,
                root,
                entry,
                file: 0,
                cache,
                _source: source,
            };
            let (file, changed) = fixture.observe(&SourceRoot::from_root(&fixture.root));
            assert!(changed);
            fixture.file = file;
            fixture.publish(file);
            fixture
        }
        pub(crate) fn observe(&mut self, source: &SourceRoot) -> (i64, bool) {
            let root = self.store.enroll_source(source).unwrap();
            let generation = self.store.begin_scan(root).unwrap();
            self.store
                .observe(
                    root,
                    generation,
                    &self.entry,
                    &std::fs::metadata(&self.entry.path).unwrap(),
                    "model",
                )
                .unwrap()
        }
        fn publish(&mut self, file: i64) {
            self.store
                .publish(
                    file,
                    self.entry.revision,
                    "model",
                    &[Segment {
                        modality: "text".into(),
                        location: "passage 0".into(),
                        text: "original content".into(),
                        input_hash: vec![1; 32],
                        vector: Some(vec![1.0; DIMENSION]),
                    }],
                    "complete",
                )
                .unwrap();
        }
        pub(crate) fn reopen(self) -> Self {
            drop(self.store); // Release the sole-writer lock before reopening.
            let store = Store::open(self.cache.path()).unwrap();
            Self { store, ..self }
        }
        fn changed_session(&self, field: &str, value: serde_json::Value) -> SourceRoot {
            let mut source = SourceRoot::from_root(&self.root);
            let mut observation = serde_json::to_value(source.session.as_ref().unwrap()).unwrap();
            observation[field] = value;
            source.session = Some(serde_json::from_value(observation).unwrap());
            source
        }
        fn legacy_version(&self, version: u32) {
            // A real legacy shape, independent of the current file-table schema.
            let (scope_column, scope_select, scope_key, sessions) = match version {
                4 => ("", "", "", "DROP TABLE source_sessions;"),
                5 => ("scope TEXT NOT NULL DEFAULT '',", ",scope", ",scope", ""),
                _ => panic!("Unsupported legacy fixture version"),
            };
            self.store.connection.execute_batch(&format!("PRAGMA foreign_keys=OFF; BEGIN;
                CREATE TABLE files_v4(id INTEGER PRIMARY KEY,device INTEGER NOT NULL,inode INTEGER NOT NULL,
                  revision INTEGER NOT NULL,pipeline TEXT NOT NULL,metadata TEXT NOT NULL,state TEXT NOT NULL DEFAULT 'pending',
                  coverage TEXT,error TEXT,extraction_version INTEGER NOT NULL DEFAULT 0,{scope_column}UNIQUE(device,inode{scope_key}));
                INSERT INTO files_v4 SELECT id,device,inode,revision,pipeline,metadata,state,coverage,error,extraction_version{scope_select} FROM files;
                DROP TABLE files; ALTER TABLE files_v4 RENAME TO files; {sessions}
                PRAGMA user_version={version}; COMMIT; PRAGMA foreign_keys=ON;")).unwrap();
        }
        fn canonical_backup(&self) -> (tempfile::NamedTempFile, u64) {
            let backup = self.cache.path().join("backup.sqlite");
            self.store.export(&backup, &AtomicBool::new(false)).unwrap();
            let input = std::fs::File::open(backup).unwrap();
            let snapshot = tempfile::NamedTempFile::new_in(self.cache.path()).unwrap();
            let staged = tempfile::NamedTempFile::new_in(self.cache.path()).unwrap();
            let vectors = super::super::restore::prepare_in_helper(
                &input,
                snapshot.as_file(),
                staged.as_file(),
                16 * 1024 * 1024,
                100,
                4096,
            )
            .unwrap();
            (staged, vectors)
        }
    }

    #[test]
    fn restart_reuses_metadata_only_with_known_local_mount_continuity() {
        let mut fixture = Fixture::new().reopen();
        let source = SourceRoot::from_root(&fixture.root);
        let persistent = source
            .session
            .as_ref()
            .is_some_and(|session| session.persistent());
        let (file, changed) = fixture.observe(&source);
        assert_eq!(changed, !persistent);
        if persistent {
            assert_eq!(file, fixture.file);
        }
    }
    #[test]
    fn changed_boot_never_reuses_same_number_object_but_retains_canonical_vectors() {
        let mut fixture = Fixture::new().reopen();
        let source = fixture.changed_session("boot", serde_json::json!(uuid::Uuid::new_v4()));
        let (file, changed) = fixture.observe(&source);
        assert!(changed);
        assert_ne!(file, fixture.file);
        assert!(
            fixture
                .store
                .reusable_vector("model", "text", &[1; 32])
                .unwrap()
                .is_some()
        );
    }
    #[test]
    fn unknown_source_observations_never_establish_metadata_continuity() {
        let mut fixture = Fixture::new();
        let mut source = SourceRoot::from_root(&fixture.root);
        source.session = None;
        let (first, _) = fixture.observe(&source);
        fixture.publish(first);
        let (second, changed) = fixture.observe(&source);
        assert!(changed);
        assert_ne!(first, second);
    }
    #[test]
    fn network_mount_reuses_within_process_but_reprepares_after_restart() {
        let mut fixture = Fixture::new();
        let source = fixture.changed_session("filesystem", serde_json::json!(0x6969));
        let (file, _) = fixture.observe(&source);
        fixture.publish(file);
        let (_, changed) = fixture.observe(&source);
        assert_eq!(changed, !source.session.as_ref().unwrap().unique());
        let mut fixture = fixture.reopen();
        let (after_restart, changed) = fixture.observe(&source);
        assert!(changed);
        assert_ne!(file, after_restart);
    }
    #[test]
    fn observed_unavailability_keeps_offline_data_but_forces_reprepare_on_reconnect() {
        let mut fixture = Fixture::new();
        fixture
            .store
            .unavailable(&fixture.root.path, "disconnected")
            .unwrap();
        let state: String = fixture
            .store
            .connection
            .query_row(
                "SELECT state FROM files WHERE id=?1",
                [fixture.file],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "ready");
        let source = SourceRoot::from_root(&fixture.root);
        let (_, changed) = fixture.observe(&source);
        assert!(changed);
    }
    #[test]
    fn version_four_upgrade_preserves_ids_vectors_and_foreign_key_enforcement() {
        let fixture = Fixture::new();
        fixture.legacy_version(4);
        let mut fixture = fixture.reopen();
        let connection = &fixture.store.connection;
        assert!(
            connection
                .pragma_query_value(None, "foreign_keys", |row| row.get::<_, bool>(0))
                .unwrap()
        );
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            super::super::SCHEMA_VERSION
        );
        assert_eq!(
            connection
                .query_row("SELECT id FROM files", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            fixture.file
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM segments", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(
            !connection
                .prepare("PRAGMA foreign_key_check")
                .unwrap()
                .exists([])
                .unwrap()
        );
        let source = SourceRoot::from_root(&fixture.root);
        let (_, changed) = fixture.observe(&source);
        assert!(changed);
        assert!(
            fixture
                .store
                .reusable_vector("model", "text", &[1; 32])
                .unwrap()
                .is_some()
        );
    }
    #[test]
    fn version_five_upgrade_and_backup_preserve_vectors_without_ambiguous_ready_state() {
        let fixture = Fixture::new();
        let original: Vec<u8> = fixture
            .store
            .connection
            .query_row("SELECT vector FROM segments", [], |row| row.get(0))
            .unwrap();
        fixture.legacy_version(5);
        let (backup, count) = fixture.canonical_backup();
        assert_eq!(count, 1);
        let imported = Connection::open(backup.path()).unwrap();
        let restored: (String, Vec<u8>, String) = imported
            .query_row("SELECT state,format,scope FROM files", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap();
        assert_eq!(
            restored,
            (
                "pending".into(),
                Vec::new(),
                format!("restored:{}", fixture.file)
            )
        );
        assert_eq!(
            imported
                .query_row("SELECT vector FROM segments", [], |row| row
                    .get::<_, Vec<u8>>(0))
                .unwrap(),
            original
        );
        let mut fixture = fixture.reopen();
        let legacy: (i64, String, Vec<u8>) = fixture
            .store
            .connection
            .query_row("SELECT id,state,format FROM files", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .unwrap();
        assert_eq!(legacy, (fixture.file, "pending".into(), Vec::new()));
        assert_eq!(
            fixture
                .store
                .connection
                .query_row("SELECT vector FROM segments", [], |row| row
                    .get::<_, Vec<u8>>(0))
                .unwrap(),
            original
        );
        let source = SourceRoot::from_root(&fixture.root);
        let (file, changed) = fixture.observe(&source);
        assert!(changed);
        assert_ne!(file, fixture.file);
        assert_eq!(
            fixture
                .store
                .reusable_vector("model", "text", &[1; 32])
                .unwrap(),
            Some(super::super::decode_vector(&original).unwrap())
        );
        fixture.publish(file);
        assert_eq!(
            fixture
                .store
                .connection
                .query_row(
                    "SELECT file_id FROM paths WHERE path=?1",
                    [fixture.entry.path.as_os_str().as_encoded_bytes()],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            file
        );
        assert!(
            fixture
                .store
                .connection
                .pragma_query_value(None, "foreign_keys", |row| row.get::<_, bool>(0))
                .unwrap()
        );
        assert!(
            !fixture
                .store
                .connection
                .prepare("PRAGMA foreign_key_check")
                .unwrap()
                .exists([])
                .unwrap()
        );
    }

    #[test]
    fn failed_v4_rebuild_rolls_back_schema_rows_and_foreign_key_setting() {
        let fixture = Fixture::new();
        fixture.legacy_version(4);
        fixture
            .store
            .connection
            .execute_batch(
                "PRAGMA foreign_keys=OFF; UPDATE paths SET file_id=999999; PRAGMA foreign_keys=ON;",
            )
            .unwrap();
        assert!(super::super::migrate(&fixture.store.connection, 4).is_err());
        let connection = &fixture.store.connection;
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            4
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM segments", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(
            connection
                .pragma_query_value(None, "foreign_keys", |row| row.get::<_, bool>(0))
                .unwrap()
        );
        assert!(connection.prepare("SELECT scope FROM files").is_err());
        assert!(connection.prepare("SELECT * FROM source_sessions").is_err());
    }
    #[test]
    fn legacy_backup_is_upgraded_without_accepting_source_continuity() {
        let fixture = Fixture::new();
        fixture.legacy_version(4);
        let (backup, vectors) = fixture.canonical_backup();
        let connection = Connection::open(backup.path()).unwrap();
        assert_eq!(vectors, 1);
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM source_sessions", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT scope FROM files", [], |row| row.get::<_, String>(0))
                .unwrap(),
            ""
        );
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            super::super::SCHEMA_VERSION
        );
    }
    #[test]
    fn backup_preserves_distinct_source_contexts_but_discards_their_authority() {
        let mut fixture = Fixture::new();
        let source = fixture.changed_session("boot", serde_json::json!(uuid::Uuid::new_v4()));
        let (file, changed) = fixture.observe(&source);
        assert!(changed);
        fixture.publish(file);
        let (backup, vectors) = fixture.canonical_backup();
        let connection = Connection::open(backup.path()).unwrap();
        assert_eq!(vectors, 2);
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM files WHERE scope LIKE 'restored:%'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM source_sessions", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
