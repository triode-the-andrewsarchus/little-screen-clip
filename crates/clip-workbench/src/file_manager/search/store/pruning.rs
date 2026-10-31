//! Bounded idle retirement after the admitted reconciliation batch has drained.
use super::{Store, record_storage_fault};
use rusqlite::params;

const PAGE: usize = 64;

#[derive(Default)]
pub(in crate::file_manager::search) struct PruneCursor {
    path: Vec<u8>,
    file: i64,
    paths_done: bool,
    pub done: bool,
}

pub(in crate::file_manager::search) struct PruneBatch {
    next: PruneCursor,
    paths: Vec<Vec<u8>>,
    file: Option<i64>,
}

impl Store {
    pub fn prune_candidates(&self, cursor: &PruneCursor) -> Result<PruneBatch, String> {
        if !cursor.paths_done {
            let mut query = self
                .connection
                .prepare(
                    "SELECT path,NOT EXISTS(SELECT 1 FROM memberships m WHERE m.path=p.path)
                FROM paths p WHERE path>?1 ORDER BY path LIMIT ?2",
                )
                .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
            let rows = query
                .query_map(params![cursor.path, PAGE], |row| {
                    Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, bool>(1)?))
                })
                .map_err(|error| record_storage_fault(&self.storage_fault, error))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
            let next = PruneCursor {
                path: rows.last().map(|row| row.0.clone()).unwrap_or_default(),
                paths_done: rows.len() < PAGE,
                ..Default::default()
            };
            return Ok(PruneBatch {
                next,
                paths: rows
                    .into_iter()
                    .filter_map(|(path, orphan)| orphan.then_some(path))
                    .collect(),
                file: None,
            });
        }
        // Canonical/imported IDs are positive. Seek by key, never repeatedly
        // scan the active prefix to find the next orphan.
        let mut query = self
            .connection
            .prepare(
                "SELECT id,NOT EXISTS(SELECT 1 FROM paths p WHERE p.file_id=f.id)
            FROM files f WHERE id>?1 ORDER BY id LIMIT ?2",
            )
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        let rows = query
            .query_map(params![cursor.file, PAGE], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, bool>(1)?))
            })
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        let mut next = PruneCursor {
            file: cursor.file,
            paths_done: true,
            ..Default::default()
        };
        let mut count = 0;
        let mut file = None;
        for row in rows {
            let (id, orphan) =
                row.map_err(|error| record_storage_fault(&self.storage_fault, error))?;
            count += 1;
            next.file = id;
            if orphan {
                file = Some(id);
                break;
            }
        }
        next.done = count < PAGE && file.is_none();
        Ok(PruneBatch {
            next,
            paths: Vec::new(),
            file,
        })
    }

    pub fn apply_prune_batch(&mut self, mut batch: PruneBatch) -> Result<PruneCursor, String> {
        if batch.paths.is_empty() && batch.file.is_none() {
            return Ok(batch.next);
        }
        let fault = &self.storage_fault;
        let tx = self
            .connection
            .transaction()
            .map_err(|error| record_storage_fault(fault, error))?;
        retire_paths(&tx, &batch.paths).map_err(|error| record_storage_fault(fault, error))?;
        let (removed_vectors, remaining) = match batch.file {
            Some(file) => {
                retire_file(&tx, file).map_err(|error| record_storage_fault(fault, error))?
            }
            None => (0, false),
        };
        if let (Some(file), true) = (batch.file, remaining) {
            batch.next.file = file - 1;
        }
        tx.commit()
            .map_err(|error| record_storage_fault(fault, error))?;
        self.vector_count = self.vector_count.saturating_sub(removed_vectors);
        Ok(batch.next)
    }
}

fn retire_paths(tx: &rusqlite::Connection, paths: &[Vec<u8>]) -> rusqlite::Result<()> {
    let mut delete=tx.prepare("DELETE FROM paths WHERE path=?1 AND NOT EXISTS(SELECT 1 FROM memberships m WHERE m.path=paths.path)")?;
    for path in paths {
        delete.execute([path])?;
    }
    Ok(())
}

fn retire_file(tx: &rusqlite::Connection, file: i64) -> rusqlite::Result<(u64, bool)> {
    let orphan: bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM files f WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM paths p WHERE p.file_id=f.id))",[file],|row|row.get(0))?;
    if !orphan {
        return Ok((0, false));
    }
    // A reappearing source must reprepare rather than trust a truncated set.
    tx.execute(
        "UPDATE files SET state='pending',coverage=NULL,error=NULL WHERE id=?1",
        [file],
    )?;
    for table in ["source_digests", "file_checkpoints", "file_retries"] {
        tx.execute(&format!("DELETE FROM {table} WHERE file_id=?1"), [file])?;
    }
    let result = retire_segments(tx, file)?;
    if !result.1 {
        tx.execute("DELETE FROM files WHERE id=?1 AND NOT EXISTS(SELECT 1 FROM paths p WHERE p.file_id=files.id)",[file])?;
    }
    Ok(result)
}

fn retire_segments(tx: &rusqlite::Connection, file: i64) -> rusqlite::Result<(u64, bool)> {
    let segments = {
        let mut query = tx.prepare(
            "SELECT id,vector IS NOT NULL FROM segments WHERE file_id=?1 ORDER BY id LIMIT ?2",
        )?;
        query
            .query_map(params![file, PAGE], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, bool>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut delete = tx.prepare("DELETE FROM segments WHERE id=?1 AND file_id=?2")?;
    let mut removed = 0;
    for (segment, vector) in segments {
        let deleted = delete.execute(params![segment, file])?;
        removed += u64::from(vector && deleted != 0);
    }
    let remaining = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM segments WHERE file_id=?1)",
        [file],
        |row| row.get(0),
    )?;
    Ok((removed, remaining))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_manager::{
        filesystem,
        search::{
            reader::SourceRoot,
            store::{Checkpoint, DIMENSION, Segment, TestFixture, source_format},
        },
    };
    use std::sync::atomic::AtomicBool;

    fn orphan_fixture() -> TestFixture {
        let mut fixture = TestFixture::new();
        let segments: Vec<_> = (0..197)
            .map(|index| Segment {
                modality: "text".into(),
                location: format!("part {index}"),
                text: format!("owned passage {index}"),
                input_hash: vec![index as u8; 32],
                vector: Some(vec![1.0; DIMENSION]),
            })
            .collect();
        fixture
            .store
            .publish(
                fixture.file,
                fixture.entry.revision,
                "model",
                &segments,
                "complete",
            )
            .unwrap();
        fixture
            .store
            .remember_source(
                fixture.file,
                fixture.entry.revision,
                "model",
                &fixture.entry,
                &[9; 32],
            )
            .unwrap();
        fixture
            .store
            .connection
            .execute("DELETE FROM memberships", [])
            .unwrap();
        fixture
    }

    #[test]
    fn idle_pruning_limits_each_segment_write_and_counts_only_committed_vectors() {
        let mut fixture = orphan_fixture();
        let mut cursor = PruneCursor::default();
        for _ in 0..20 {
            let before = fixture.store.vector_count;
            let batch = fixture.store.prune_candidates(&cursor).unwrap();
            cursor = fixture.store.apply_prune_batch(batch).unwrap();
            assert!(before - fixture.store.vector_count <= PAGE as u64);
            if cursor.done {
                break;
            }
        }
        assert!(cursor.done);
        assert_eq!(fixture.store.vector_count, 0);
        let files: usize = fixture
            .store
            .connection
            .query_row("SELECT count(*) FROM files", [], |row| row.get(0))
            .unwrap();
        assert_eq!(files, 0);
        assert_eq!(
            std::fs::read(&fixture.entry.path).unwrap(),
            b"original content"
        );
        fixture
            .store
            .connection
            .execute(
                "INSERT INTO segment_fts(segment_fts,rank) VALUES('integrity-check',1)",
                [],
            )
            .unwrap();
    }

    #[test]
    fn partial_prune_reappearance_reprepares_instead_of_claiming_complete_source_reuse() {
        let mut fixture = orphan_fixture();
        let batch = fixture
            .store
            .prune_candidates(&PruneCursor::default())
            .unwrap();
        let cursor = fixture.store.apply_prune_batch(batch).unwrap();
        let batch = fixture.store.prune_candidates(&cursor).unwrap();
        let mut cursor = fixture.store.apply_prune_batch(batch).unwrap();
        assert_eq!(fixture.store.vector_count, 197 - PAGE as u64);
        let (file, pending) = fixture.observe(&SourceRoot::from_root(&fixture.root));
        assert_eq!(file, fixture.file);
        assert!(
            pending,
            "partially deleted data was mistaken for a complete record"
        );
        assert!(
            fixture
                .store
                .reusable_source(&fixture.entry, "model", &[9; 32])
                .unwrap()
                .is_none()
        );
        for _ in 0..10 {
            let batch = fixture.store.prune_candidates(&cursor).unwrap();
            cursor = fixture.store.apply_prune_batch(batch).unwrap();
            if cursor.done {
                break;
            }
        }
        assert!(cursor.done);
        assert_eq!(
            fixture.store.vector_count,
            197 - PAGE as u64,
            "reappeared input was deleted"
        );
    }

    #[test]
    fn candidate_application_rechecks_path_and_file_ownership() {
        for file_phase in [false, true] {
            let mut fixture = orphan_fixture();
            let mut batch = fixture
                .store
                .prune_candidates(&PruneCursor::default())
                .unwrap();
            if file_phase {
                let cursor = fixture.store.apply_prune_batch(batch).unwrap();
                batch = fixture.store.prune_candidates(&cursor).unwrap();
            }
            fixture.observe(&SourceRoot::from_root(&fixture.root));
            fixture.store.apply_prune_batch(batch).unwrap();
            assert_eq!(fixture.store.vector_count, 197);
            assert!(
                fixture
                    .store
                    .reusable_source(&fixture.entry, "model", &[9; 32])
                    .unwrap()
                    .is_some()
            );
        }
    }

    #[test]
    fn checkpoint_replacement_revokes_old_whole_source_digest_atomically() {
        let mut fixture = TestFixture::new();
        fixture
            .store
            .remember_source(
                fixture.file,
                fixture.entry.revision,
                "model",
                &fixture.entry,
                &[9; 32],
            )
            .unwrap();
        std::fs::write(&fixture.entry.path, "different source bytes now").unwrap();
        fixture.entry = filesystem::read_directory(&fixture.root.path, &AtomicBool::new(false))
            .unwrap()
            .entries
            .remove(0);
        fixture.observe(&SourceRoot::from_root(&fixture.root));
        assert!(
            fixture
                .store
                .reusable_source(&fixture.entry, "model", &[9; 32])
                .unwrap()
                .is_some(),
            "old canonical input should remain reusable before any replacement"
        );
        let checkpoint = Checkpoint {
            cursor: 1,
            text_cursor: 0,
            coverage: "complete".into(),
            format: source_format(&fixture.entry),
            digest: Some([8; 32]),
            prepared_hash: None,
            segments: 0,
        };
        let replacement = Segment {
            modality: "text".into(),
            location: "new part".into(),
            text: "different".into(),
            input_hash: vec![7; 32],
            vector: Some(vec![1.0; DIMENSION]),
        };
        fixture
            .store
            .checkpoint(
                fixture.file,
                fixture.entry.revision,
                "model",
                &[replacement],
                &checkpoint,
            )
            .unwrap();
        assert!(
            fixture
                .store
                .reusable_source(&fixture.entry, "model", &[9; 32])
                .unwrap()
                .is_none(),
            "old digest authorized a new partial segment set"
        );
    }
}
