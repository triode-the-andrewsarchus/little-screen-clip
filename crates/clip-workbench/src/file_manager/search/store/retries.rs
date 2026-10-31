//! Persistent, bounded retries for one canonical source representation.
use super::{EXTRACTION_VERSION, Store, record_storage_fault};
use rusqlite::{OptionalExtension, params};
use std::time::Duration;

// Operational state, deliberately not restored from imported catalogs. Source
// identity and canonical vectors keep their existing schema/backup contracts.
pub(super) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS file_retries(
    file_id INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL, pipeline TEXT NOT NULL, extraction_version INTEGER NOT NULL,
    failures INTEGER NOT NULL CHECK(failures BETWEEN 1 AND 4), retry_after INTEGER
);";

#[derive(Debug, PartialEq, Eq)]
pub(in crate::file_manager::search) enum Retry {
    Ready,
    After(Duration),
    Stopped,
}

impl Store {
    pub fn file_retry(&self, file: i64) -> Result<Retry, String> {
        let remaining: Option<Option<u64>> = self.connection.query_row(
            "SELECT CASE WHEN retry_after IS NULL THEN NULL ELSE max(0,retry_after-unixepoch()) END FROM file_retries WHERE file_id=?1",
            [file], |row| row.get(0)).optional().map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        Ok(match remaining {
            None | Some(Some(0)) => Retry::Ready,
            Some(Some(seconds)) => Retry::After(Duration::from_secs(seconds.min(1800))),
            Some(None) => Retry::Stopped,
        })
    }

    pub fn record_file_failure(
        &mut self,
        file: i64,
        revision: u64,
        pipeline: &str,
        error: &str,
        partial: bool,
    ) -> Result<bool, String> {
        let fault = &self.storage_fault;
        let tx = self
            .connection
            .transaction()
            .map_err(|error| record_storage_fault(fault, error))?;
        let current = tx
            .execute(
                "UPDATE files SET state=CASE WHEN ?5 THEN state ELSE 'failed' END,error=?4
            WHERE id=?1 AND revision=?2 AND pipeline=?3 AND extraction_version=?6",
                params![
                    file,
                    revision as i64,
                    pipeline,
                    error,
                    partial,
                    EXTRACTION_VERSION
                ],
            )
            .map_err(|error| record_storage_fault(fault, error))?;
        if current == 0 {
            return Ok(false);
        }
        // A second alias/completion during the same backoff cannot consume the
        // next attempt. The fourth failure waits for change or manual rebuild.
        tx.execute("INSERT INTO file_retries(file_id,revision,pipeline,extraction_version,failures,retry_after)
            VALUES(?1,?2,?3,?4,1,unixepoch()+60)
            ON CONFLICT(file_id) DO UPDATE SET failures=min(4,failures+1),
                retry_after=CASE failures WHEN 1 THEN unixepoch()+300 WHEN 2 THEN unixepoch()+1800 ELSE NULL END
            WHERE retry_after IS NOT NULL AND retry_after<=unixepoch()",
            params![file, revision as i64, pipeline, EXTRACTION_VERSION])
            .map_err(|error| record_storage_fault(fault, error))?;
        tx.commit()
            .map_err(|error| record_storage_fault(fault, error))?;
        Ok(true)
    }

    pub fn clear_file_retry(&self, file: i64) -> Result<(), String> {
        self.connection
            .execute("DELETE FROM file_retries WHERE file_id=?1", [file])
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        Ok(())
    }

    pub fn reset_retries(&mut self, root: i64) -> Result<(), String> {
        let fault = &self.storage_fault;
        let tx = self
            .connection
            .transaction()
            .map_err(|error| record_storage_fault(fault, error))?;
        // A failed file may already have a ready partial publication. Removing
        // its retry record alone would incorrectly make observation skip it.
        tx.execute("UPDATE files SET state='pending' WHERE id IN (SELECT file_id FROM file_retries)
            AND id IN (SELECT p.file_id FROM paths p JOIN memberships m ON m.path=p.path WHERE m.root_id=?1)", [root])
            .map_err(|error| record_storage_fault(fault, error))?;
        tx.execute(
            "DELETE FROM file_retries WHERE file_id IN
            (SELECT p.file_id FROM paths p JOIN memberships m ON m.path=p.path WHERE m.root_id=?1)",
            [root],
        )
        .map_err(|error| record_storage_fault(fault, error))?;
        tx.commit()
            .map_err(|error| record_storage_fault(fault, error))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_manager::search::{reader::SourceRoot, store::TestFixture};

    #[test]
    fn retries_survive_reopen_and_stop_after_three_delayed_attempts() {
        let mut fixture = TestFixture::new();
        assert_eq!(
            fixture.store.file_retry(fixture.file).unwrap(),
            Retry::Ready
        );
        for (attempt, delay) in [(1, Some(60)), (2, Some(300)), (3, Some(1800)), (4, None)] {
            assert!(
                fixture
                    .store
                    .record_file_failure(
                        fixture.file,
                        fixture.entry.revision,
                        "model",
                        "fixture failure",
                        false
                    )
                    .unwrap()
            );
            // A duplicate hard-link completion must not consume another retry.
            fixture
                .store
                .record_file_failure(
                    fixture.file,
                    fixture.entry.revision,
                    "model",
                    "duplicate",
                    false,
                )
                .unwrap();
            fixture = fixture.reopen();
            let (failures, seconds): (u32, Option<u64>) = fixture
                .store
                .connection
                .query_row(
                    "SELECT failures,retry_after-unixepoch() FROM file_retries WHERE file_id=?1",
                    [fixture.file],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(failures, attempt);
            match delay {
                Some(delay) => {
                    assert!((delay - 1..=delay).contains(&seconds.unwrap()));
                    assert!(matches!(
                        fixture.store.file_retry(fixture.file).unwrap(),
                        Retry::After(_)
                    ));
                    fixture
                        .store
                        .connection
                        .execute(
                            "UPDATE file_retries SET retry_after=unixepoch()-1 WHERE file_id=?1",
                            [fixture.file],
                        )
                        .unwrap();
                    assert_eq!(
                        fixture.store.file_retry(fixture.file).unwrap(),
                        Retry::Ready
                    );
                }
                None => {
                    assert!(seconds.is_none());
                    assert_eq!(
                        fixture.store.file_retry(fixture.file).unwrap(),
                        Retry::Stopped
                    );
                }
            }
        }
        let root = fixture.store.roots().unwrap()[0].id;
        fixture.store.reset_retries(root).unwrap();
        assert_eq!(
            fixture.store.file_retry(fixture.file).unwrap(),
            Retry::Ready
        );
    }

    #[test]
    fn source_or_pipeline_change_resets_retry_but_stale_failure_cannot_poison_it() {
        for changed_pipeline in [false, true] {
            let mut fixture = TestFixture::new();
            let original = fixture.entry.revision;
            fixture
                .store
                .record_file_failure(fixture.file, original, "model", "failed", false)
                .unwrap();
            let root = fixture.store.roots().unwrap()[0].id;
            let pipeline = if changed_pipeline {
                "new-model"
            } else {
                "model"
            };
            if !changed_pipeline {
                fixture.entry.revision += 1;
            }
            let (file, pending) = fixture
                .store
                .observe_object(
                    root,
                    1,
                    &fixture.entry,
                    fixture.entry.identity.unwrap().object(),
                    pipeline,
                )
                .unwrap();
            assert_eq!(file, fixture.file);
            assert!(pending);
            assert_eq!(fixture.store.file_retry(file).unwrap(), Retry::Ready);
            assert!(
                !fixture
                    .store
                    .record_file_failure(file, original, "model", "late old failure", false)
                    .unwrap()
            );
            assert_eq!(fixture.store.file_retry(file).unwrap(), Retry::Ready);
        }
    }

    #[test]
    fn partial_failures_keep_vectors_but_remain_retryable_and_root_partial() {
        let mut fixture = TestFixture::new();
        fixture
            .store
            .record_file_failure(
                fixture.file,
                fixture.entry.revision,
                "model",
                "late decoder failure",
                true,
            )
            .unwrap();
        let root = fixture.store.roots().unwrap()[0].id;
        fixture.store.finish_scan(root, None).unwrap();
        assert_eq!(fixture.store.roots().unwrap()[0].state, "partial");
        let (file, pending) = fixture.observe(&SourceRoot::from_root(&fixture.root));
        assert_eq!(file, fixture.file);
        assert!(pending, "ready partial failure lost its retry");
        assert!(matches!(
            fixture.store.file_retry(file).unwrap(),
            Retry::After(_)
        ));
        let count: usize = fixture
            .store
            .connection
            .query_row(
                "SELECT count(*) FROM segments WHERE vector IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        fixture.store.reset_retries(root).unwrap();
        let (_, pending) = fixture.observe(&SourceRoot::from_root(&fixture.root));
        assert!(pending, "manual retry skipped a ready partial publication");
        fixture
            .store
            .record_file_failure(file, fixture.entry.revision, "model", "failed again", false)
            .unwrap();
        fixture.store.remove_root(root).unwrap();
        let retries: usize = fixture
            .store
            .connection
            .query_row("SELECT count(*) FROM file_retries", [], |row| row.get(0))
            .unwrap();
        assert_eq!(retries, 0, "orphan retry records outlived canonical files");
    }
}
