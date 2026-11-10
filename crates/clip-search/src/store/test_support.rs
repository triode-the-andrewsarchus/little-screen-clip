//! Owned index fixtures shared by backend and browser contract tests.
use super::*;
use crate::{policy::Root, source::SourceRoot};
use little_clip_core::file_inventory::{self as filesystem, Entry};
use std::sync::atomic::AtomicBool;

pub struct Fixture {
    pub store: Store,
    pub root: Root,
    pub entry: Entry,
    pub file: i64,
    cache: tempfile::TempDir,
    _source: tempfile::TempDir,
}
impl Fixture {
    pub fn new() -> Self {
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
    pub fn without_memberships(self) -> Self {
        self.store
            .connection
            .execute("DELETE FROM memberships", [])
            .unwrap();
        self
    }

    pub fn observe(&mut self, source: &SourceRoot) -> (i64, bool) {
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
    pub(crate) fn publish(&mut self, file: i64) {
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
    pub fn reopen(self) -> Self {
        drop(self.store); // Release the sole-writer lock before reopening.
        let store = Store::open(self.cache.path()).unwrap();
        Self { store, ..self }
    }
    #[cfg(test)]
    pub(crate) fn changed_session(&self, field: &str, value: serde_json::Value) -> SourceRoot {
        let mut source = SourceRoot::from_root(&self.root);
        let mut observation = serde_json::to_value(source.session.as_ref().unwrap()).unwrap();
        observation[field] = value;
        source.session = Some(serde_json::from_value(observation).unwrap());
        source
    }
    #[cfg(test)]
    pub(crate) fn legacy_version(&self, version: u32) {
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
    #[cfg(test)]
    pub(crate) fn canonical_backup(&self) -> (tempfile::NamedTempFile, u64) {
        let backup = self.cache.path().join("backup.sqlite");
        self.store.export(&backup, &AtomicBool::new(false)).unwrap();
        let input = std::fs::File::open(backup).unwrap();
        let snapshot = tempfile::NamedTempFile::new_in(self.cache.path()).unwrap();
        let staged = tempfile::NamedTempFile::new_in(self.cache.path()).unwrap();
        let vectors = super::restore::prepare_in_helper(
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

impl Default for Fixture {
    fn default() -> Self {
        Self::new()
    }
}
