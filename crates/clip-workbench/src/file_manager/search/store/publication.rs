//! Atomic complete publications and resumable, source-bound partial batches.
use super::{
    EXTRACTION_VERSION, Segment, Store, encode_vector, fingerprints, record_storage_fault,
};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub(super) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS file_checkpoints(
    file_id INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL, pipeline TEXT NOT NULL, extraction_version INTEGER NOT NULL,
    progress TEXT NOT NULL CHECK(length(progress)<=4096),
    segments INTEGER NOT NULL CHECK(segments BETWEEN 1 AND 2048)
);";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::file_manager::search) struct Checkpoint {
    pub cursor: usize,
    pub text_cursor: usize,
    pub coverage: String,
    pub format: Vec<u8>,
    pub digest: Option<[u8; 32]>,
    pub prepared_hash: Option<[u8; 32]>,
    // Expected committed prefix when appending; durable count is a SQL column.
    #[serde(skip)]
    pub segments: usize,
}
impl Checkpoint {
    fn encode(&self) -> Result<String, String> {
        let json = serde_json::to_string(self).map_err(|error| error.to_string())?;
        if json.len() > 4096 {
            return Err("Index checkpoint exceeds its metadata budget".into());
        }
        Ok(json)
    }

    fn valid(&self) -> bool {
        self.cursor <= 64 * 1024 * 1024
            && self.text_cursor <= 8 * 1024 * 1024
            && self.coverage.len() <= 3072
            && self.format.len() <= 512
            && (self.text_cursor > 0) == self.prepared_hash.is_some()
    }
}

impl Store {
    pub fn publish(
        &mut self,
        file: i64,
        revision: u64,
        pipeline: &str,
        segments: &[Segment],
        coverage: &str,
    ) -> Result<bool, String> {
        self.commit_segments(file, revision, pipeline, segments, coverage, None)
    }

    pub fn checkpoint(
        &mut self,
        file: i64,
        revision: u64,
        pipeline: &str,
        segments: &[Segment],
        progress: &Checkpoint,
    ) -> Result<bool, String> {
        if !progress.valid() || segments.is_empty() || segments.len() > 2048 {
            return Err("Invalid indexing checkpoint".into());
        }
        let coverage = format!(
            "partial: indexing in progress; {} indexed parts; planned coverage: {}",
            segments.len(),
            progress.coverage
        );
        self.commit_segments(
            file,
            revision,
            pipeline,
            segments,
            &coverage,
            Some(progress),
        )
    }

    fn commit_segments(
        &mut self,
        file: i64,
        revision: u64,
        pipeline: &str,
        segments: &[Segment],
        coverage: &str,
        progress: Option<&Checkpoint>,
    ) -> Result<bool, String> {
        let from = match progress {
            Some(progress) => progress.segments,
            None => matching_prefix(&self.connection, file, revision, pipeline, segments)?,
        };
        let tail = segments.get(from..).ok_or("Invalid checkpoint prefix")?;
        // Keep the original publication contract: validate every supplied
        // vector before any write, even when stored prefix bytes are retained.
        let encoded: Vec<_> = segments
            .iter()
            .map(|segment| segment.vector.as_deref().map(encode_vector).transpose())
            .collect::<Result<_, _>>()?;
        let json = progress.map(Checkpoint::encode).transpose()?;
        let fault = &self.storage_fault;
        let tx = self
            .connection
            .transaction()
            .map_err(|e| record_storage_fault(fault, e))?;
        let current: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM files WHERE id=?1 AND revision=?2 AND pipeline=?3)",
                params![file, revision as i64, pipeline],
                |r| r.get(0),
            )
            .map_err(|e| record_storage_fault(fault, e))?;
        if !current || !prefix_matches(&tx, file, revision, pipeline, from, fault)? {
            return Ok(false);
        }
        let old = super::file_vectors(&tx, file, fault)?;
        append_segments(&tx, file, pipeline, tail, &encoded[from..], from == 0)
            .map_err(|e| record_storage_fault(fault, e))?;
        write_status(
            &tx,
            file,
            revision,
            pipeline,
            segments.len(),
            coverage,
            json.as_deref(),
        )
        .map_err(|e| record_storage_fault(fault, e))?;
        tx.commit().map_err(|e| record_storage_fault(fault, e))?;
        self.vector_count = self.vector_count.saturating_sub(old).saturating_add(
            segments
                .iter()
                .filter(|segment| segment.vector.is_some())
                .count() as u64,
        );
        Ok(true)
    }

    pub fn resume_file(
        &self,
        file: i64,
        revision: u64,
        pipeline: &str,
        entry: &crate::file_manager::filesystem::Entry,
        digest: Option<[u8; 32]>,
    ) -> Result<Option<(Checkpoint, Vec<Segment>)>, String> {
        let saved: Option<(String, usize)> = self.connection.query_row(
            "SELECT c.progress,c.segments FROM file_checkpoints c JOIN files f ON f.id=c.file_id
             WHERE c.file_id=?1 AND c.revision=?2 AND c.pipeline=?3 AND c.extraction_version=?4
             AND f.revision=c.revision AND f.pipeline=c.pipeline AND f.extraction_version=c.extraction_version",
            params![file,revision as i64,pipeline,EXTRACTION_VERSION], |r| Ok((r.get(0)?,r.get(1)?)),
        ).optional().map_err(|e| record_storage_fault(&self.storage_fault, e))?;
        let Some((json, count)) = saved else {
            return Ok(None);
        };
        let mut progress: Checkpoint =
            serde_json::from_str(&json).map_err(|_| "Invalid saved checkpoint")?;
        if !progress.valid()
            || progress.format != fingerprints::format(entry)
            || progress.digest != digest
        {
            self.discard_checkpoint(file, revision, pipeline)?;
            return Ok(None);
        }
        let segments = fingerprints::read_segments(&self.connection, file, pipeline)?;
        if segments.len() != count {
            return Err("Incomplete checkpoint batch".into());
        }
        progress.segments = count;
        Ok(Some((progress, segments)))
    }

    pub fn discard_checkpoint(
        &self,
        file: i64,
        revision: u64,
        pipeline: &str,
    ) -> Result<(), String> {
        let fault = &self.storage_fault;
        let tx = self
            .connection
            .unchecked_transaction()
            .map_err(|e| record_storage_fault(fault, e))?;
        tx.execute(
            "UPDATE files SET state='pending' WHERE id=?1 AND revision=?2 AND pipeline=?3",
            params![file, revision as i64, pipeline],
        )
        .map_err(|e| record_storage_fault(fault, e))?;
        tx.execute(
            "DELETE FROM file_checkpoints WHERE file_id=?1 AND revision=?2 AND pipeline=?3",
            params![file, revision as i64, pipeline],
        )
        .map_err(|e| record_storage_fault(fault, e))?;
        tx.commit().map_err(|e| record_storage_fault(fault, e))
    }
}

fn append_segments(
    tx: &rusqlite::Connection,
    file: i64,
    pipeline: &str,
    segments: &[Segment],
    encoded: &[Option<Vec<u8>>],
    replace: bool,
) -> rusqlite::Result<()> {
    tx.execute("DELETE FROM source_digests WHERE file_id=?1", [file])?;
    if replace {
        tx.execute("DELETE FROM segments WHERE file_id=?1", [file])?;
    }
    for (segment, vector) in segments.iter().zip(encoded) {
        tx.execute("INSERT INTO segments(file_id,modality,location,text,input_hash,pipeline,vector) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![file,segment.modality,segment.location,segment.text,segment.input_hash,pipeline,vector])?;
    }
    Ok(())
}

fn write_status(
    tx: &rusqlite::Connection,
    file: i64,
    revision: u64,
    pipeline: &str,
    count: usize,
    coverage: &str,
    json: Option<&str>,
) -> rusqlite::Result<()> {
    tx.execute(
        "UPDATE files SET state=?2,coverage=?3,error=NULL WHERE id=?1",
        params![
            file,
            if count == 0 { "unsupported" } else { "ready" },
            coverage
        ],
    )?;
    if let Some(json) = json {
        tx.execute("INSERT OR REPLACE INTO file_checkpoints(file_id,revision,pipeline,extraction_version,progress,segments) VALUES(?1,?2,?3,?4,?5,?6)",
            params![file,revision as i64,pipeline,EXTRACTION_VERSION,json,count])?;
    } else {
        tx.execute("DELETE FROM file_checkpoints WHERE file_id=?1", [file])?;
    }
    Ok(())
}

// Preserve canonical half bytes (and IDs) instead of repeatedly quantizing the
// completed prefix after every restart. Changed inputs/locations replace it.
fn matching_prefix(
    connection: &rusqlite::Connection,
    file: i64,
    revision: u64,
    pipeline: &str,
    segments: &[Segment],
) -> Result<usize, String> {
    let count: Option<usize> = connection.query_row(
        "SELECT segments FROM file_checkpoints WHERE file_id=?1 AND revision=?2 AND pipeline=?3 AND extraction_version=?4",
        params![file,revision as i64,pipeline,EXTRACTION_VERSION], |r| r.get(0),
    ).optional().map_err(|e| e.to_string())?;
    let Some(count) = count.filter(|&n| n <= segments.len()) else {
        return Ok(0);
    };
    let mut statement = connection.prepare("SELECT modality,location,text,input_hash,vector IS NOT NULL FROM segments WHERE file_id=?1 ORDER BY id LIMIT ?2").map_err(|e| e.to_string())?;
    let mut rows = statement
        .query(params![file, count])
        .map_err(|e| e.to_string())?;
    for segment in &segments[..count] {
        let Some(row) = rows.next().map_err(|e| e.to_string())? else {
            return Ok(0);
        };
        let stored: (String, String, String, Vec<u8>, bool) = (
            row.get(0).map_err(|e| e.to_string())?,
            row.get(1).map_err(|e| e.to_string())?,
            row.get(2).map_err(|e| e.to_string())?,
            row.get(3).map_err(|e| e.to_string())?,
            row.get(4).map_err(|e| e.to_string())?,
        );
        if (
            stored.0.as_str(),
            stored.1.as_str(),
            stored.2.as_str(),
            stored.3.as_slice(),
            stored.4,
        ) != (
            segment.modality.as_str(),
            segment.location.as_str(),
            segment.text.as_str(),
            segment.input_hash.as_slice(),
            segment.vector.is_some(),
        ) {
            return Ok(0);
        }
    }
    Ok(count)
}

fn prefix_matches(
    tx: &rusqlite::Connection,
    file: i64,
    revision: u64,
    pipeline: &str,
    count: usize,
    fault: &std::cell::Cell<bool>,
) -> Result<bool, String> {
    if count == 0 {
        return Ok(true);
    }
    tx.query_row("SELECT EXISTS(SELECT 1 FROM file_checkpoints c WHERE file_id=?1 AND revision=?2 AND pipeline=?3 AND extraction_version=?4 AND segments=?5 AND (SELECT COUNT(*) FROM segments WHERE file_id=c.file_id)=?5)",
        params![file,revision as i64,pipeline,EXTRACTION_VERSION,count], |r| r.get(0)).map_err(|e| record_storage_fault(fault, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_manager::search::{reader::SourceRoot, store::session::tests::Fixture};

    fn progress(fixture: &Fixture) -> Checkpoint {
        Checkpoint {
            cursor: 0,
            text_cursor: 5,
            coverage: "complete".into(),
            format: fingerprints::format(&fixture.entry),
            digest: Some([7; 32]),
            prepared_hash: Some([9; 32]),
            segments: 0,
        }
    }
    fn batch(count: usize) -> Vec<Segment> {
        (0..count)
            .map(|index| {
                let mut vector = vec![0.0; super::super::DIMENSION];
                vector[0] = 0.1234567;
                vector[1] = 0.9876543;
                Segment {
                    modality: "text".into(),
                    location: format!("part {index}"),
                    text: format!("passage {index}"),
                    input_hash: vec![index as u8; 32],
                    vector: Some(vector),
                }
            })
            .collect()
    }
    fn stored(fixture: &Fixture) -> Vec<(i64, Option<Vec<u8>>)> {
        fixture
            .store
            .connection
            .prepare("SELECT id,vector FROM segments WHERE file_id=?1 ORDER BY id")
            .unwrap()
            .query_map([fixture.file], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn partial_fixture() -> Fixture {
        let mut fixture = Fixture::new();
        let point = progress(&fixture);
        assert!(
            fixture
                .store
                .checkpoint(
                    fixture.file,
                    fixture.entry.revision,
                    "model",
                    &batch(8),
                    &point
                )
                .unwrap()
        );
        fixture
    }

    #[test]
    fn restart_resumes_a_partial_batch_and_finalization_preserves_canonical_prefix() {
        let fixture = partial_fixture();
        let prefix = stored(&fixture);
        let root = fixture.store.roots().unwrap()[0].id;
        fixture.store.finish_scan(root, None).unwrap();
        assert_eq!(fixture.store.roots().unwrap()[0].state, "building");
        let coverage: String = fixture
            .store
            .connection
            .query_row(
                "SELECT coverage FROM files WHERE id=?1",
                [fixture.file],
                |r| r.get(0),
            )
            .unwrap();
        assert!(coverage.starts_with("partial: indexing in progress"));

        let mut fixture = fixture.reopen();
        let (file, pending) = fixture.observe(&SourceRoot::from_root(&fixture.root));
        assert!(pending, "a durable partial file must be scheduled again");
        assert_eq!(file, fixture.file);
        let (point, mut segments) = fixture
            .store
            .resume_file(
                file,
                fixture.entry.revision,
                "model",
                &fixture.entry,
                Some([7; 32]),
            )
            .unwrap()
            .unwrap();
        assert_eq!((point.segments, point.cursor, point.text_cursor), (8, 0, 5));
        segments.extend(batch(16).into_iter().skip(8));
        assert!(
            fixture
                .store
                .checkpoint(file, fixture.entry.revision, "model", &segments, &point)
                .unwrap()
        );
        assert_eq!(&stored(&fixture)[..8], prefix.as_slice());
        assert_eq!(fixture.store.vector_count, 16);
        assert!(
            !fixture
                .store
                .checkpoint(file, fixture.entry.revision, "model", &segments, &point)
                .unwrap(),
            "stale append prefix cannot duplicate passages"
        );
        let (_, restored) = fixture
            .store
            .resume_file(
                file,
                fixture.entry.revision,
                "model",
                &fixture.entry,
                Some([7; 32]),
            )
            .unwrap()
            .unwrap();
        let completed = stored(&fixture);
        assert!(
            fixture
                .store
                .publish(file, fixture.entry.revision, "model", &restored, "complete")
                .unwrap()
        );
        assert_eq!(
            stored(&fixture),
            completed,
            "finishing cannot requantize canonical half bytes"
        );
        assert!(
            fixture
                .store
                .resume_file(
                    file,
                    fixture.entry.revision,
                    "model",
                    &fixture.entry,
                    Some([7; 32])
                )
                .unwrap()
                .is_none()
        );
        fixture.store.finish_scan(root, None).unwrap();
        assert_eq!(fixture.store.roots().unwrap()[0].state, "ready");
        assert!(!fixture.observe(&SourceRoot::from_root(&fixture.root)).1);
    }

    #[test]
    fn failed_checkpoint_commit_rolls_back_segments_cursor_and_vector_count_together() {
        let mut fixture = partial_fixture();
        let original = stored(&fixture);
        let mut invalid = batch(8);
        invalid[0].vector.as_mut().unwrap()[0] = f32::NAN;
        assert!(
            fixture
                .store
                .publish(
                    fixture.file,
                    fixture.entry.revision,
                    "model",
                    &invalid,
                    "complete"
                )
                .is_err()
        );
        assert_eq!(
            stored(&fixture),
            original,
            "even a retained prefix must reject invalid supplied vectors"
        );
        let (point, _) = fixture
            .store
            .resume_file(
                fixture.file,
                fixture.entry.revision,
                "model",
                &fixture.entry,
                Some([7; 32]),
            )
            .unwrap()
            .unwrap();
        fixture.store.connection.execute_batch("CREATE TRIGGER fail_checkpoint BEFORE INSERT ON file_checkpoints BEGIN SELECT RAISE(ABORT,'injected checkpoint failure'); END;").unwrap();
        assert!(
            fixture
                .store
                .checkpoint(
                    fixture.file,
                    fixture.entry.revision,
                    "model",
                    &batch(16),
                    &point
                )
                .is_err()
        );
        assert_eq!(stored(&fixture), original);
        assert_eq!(fixture.store.vector_count, 8);
        assert_eq!(
            fixture
                .store
                .resume_file(
                    fixture.file,
                    fixture.entry.revision,
                    "model",
                    &fixture.entry,
                    Some([7; 32])
                )
                .unwrap()
                .unwrap()
                .0
                .segments,
            8
        );
    }

    #[test]
    fn incompatible_source_digest_or_dispatch_retires_the_cursor_and_hides_old_partial_results() {
        for (extension, saved_format, typescript, digest) in [
            ("txt", b"txt".as_slice(), false, [8; 32]),
            ("html", b"txt".as_slice(), false, [7; 32]),
            ("ts", b"ts".as_slice(), true, [7; 32]),
            ("ts", b"ts\0\x01\0".as_slice(), true, [7; 32]),
        ] {
            let fixture = partial_fixture();
            let mut entry = fixture.entry.clone();
            entry.path.set_extension(extension);
            entry.typescript = typescript;
            let json: String = fixture
                .store
                .connection
                .query_row(
                    "SELECT progress FROM file_checkpoints WHERE file_id=?1",
                    [fixture.file],
                    |row| row.get(0),
                )
                .unwrap();
            let mut saved: Checkpoint = serde_json::from_str(&json).unwrap();
            saved.format = saved_format.to_vec();
            fixture
                .store
                .connection
                .execute(
                    "UPDATE file_checkpoints SET progress=?2 WHERE file_id=?1",
                    params![fixture.file, saved.encode().unwrap()],
                )
                .unwrap();
            let digest = Some(digest);
            assert!(
                fixture
                    .store
                    .resume_file(fixture.file, entry.revision, "model", &entry, digest)
                    .unwrap()
                    .is_none()
            );
            let (state, count): (String, usize) = fixture
                .store
                .connection
                .query_row(
                    "SELECT state,(SELECT COUNT(*) FROM file_checkpoints) FROM files WHERE id=?1",
                    [fixture.file],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!((state.as_str(), count), ("pending", 0));
            assert_eq!(
                stored(&fixture).len(),
                8,
                "canonical inputs remain available for verified-input reuse"
            );
        }
    }

    #[test]
    fn changed_revision_pipeline_and_extractor_cannot_resume_or_publish_a_stale_cursor() {
        for change in ["revision", "pipeline", "extractor"] {
            let mut fixture = partial_fixture();
            let pipeline = if change == "pipeline" {
                "new-model"
            } else {
                "model"
            };
            if change == "revision" {
                fixture.entry.revision = fixture.entry.revision.wrapping_add(1);
            }
            if change == "extractor" {
                fixture.store.connection.execute_batch("UPDATE files SET extraction_version=0; UPDATE file_checkpoints SET extraction_version=0;").unwrap();
            }
            let root = fixture.store.roots().unwrap()[0].id;
            let generation = fixture.store.begin_scan(root).unwrap();
            fixture
                .store
                .observe(
                    root,
                    generation,
                    &fixture.entry,
                    &std::fs::metadata(&fixture.entry.path).unwrap(),
                    pipeline,
                )
                .unwrap();
            assert!(
                fixture
                    .store
                    .resume_file(
                        fixture.file,
                        fixture.entry.revision,
                        pipeline,
                        &fixture.entry,
                        Some([7; 32])
                    )
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                fixture
                    .store
                    .connection
                    .query_row("SELECT COUNT(*) FROM file_checkpoints", [], |r| r
                        .get::<_, usize>(0))
                    .unwrap(),
                0
            );
        }
    }

    #[test]
    fn a_cursor_without_a_full_digest_still_requires_the_same_verified_source_scope() {
        let mut fixture = Fixture::new();
        let mut point = progress(&fixture);
        point.cursor = 200;
        point.text_cursor = 0;
        point.prepared_hash = None;
        point.digest = None;
        fixture
            .store
            .checkpoint(
                fixture.file,
                fixture.entry.revision,
                "model",
                &batch(8),
                &point,
            )
            .unwrap();
        let mut fixture = fixture.reopen();
        let (saved, _) = fixture
            .store
            .resume_file(
                fixture.file,
                fixture.entry.revision,
                "model",
                &fixture.entry,
                None,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            (saved.cursor, saved.text_cursor, saved.prepared_hash),
            (200, 0, None)
        );
        // Uncertain mount continuity is a new file scope, even if ordinary
        // metadata and the absence of a full-source digest appear unchanged.
        let mut uncertain = SourceRoot::from_root(&fixture.root);
        uncertain.session = None;
        let (file, pending) = fixture.observe(&uncertain);
        assert!(pending);
        assert_ne!(file, fixture.file);
        assert!(
            fixture
                .store
                .resume_file(file, fixture.entry.revision, "model", &fixture.entry, None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn finalization_can_add_vectors_to_a_lexical_only_prefix() {
        let mut fixture = partial_fixture();
        let mut text_only = batch(8);
        for segment in &mut text_only {
            segment.vector = None;
        }
        let point = progress(&fixture);
        fixture
            .store
            .checkpoint(
                fixture.file,
                fixture.entry.revision,
                "model",
                &text_only,
                &point,
            )
            .unwrap();
        assert_eq!(fixture.store.vector_count, 0);
        fixture
            .store
            .publish(
                fixture.file,
                fixture.entry.revision,
                "model",
                &batch(8),
                "complete",
            )
            .unwrap();
        let actual: u64 = fixture
            .store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM segments WHERE vector IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            actual, 8,
            "matching text cannot preserve a missing vector when finalization supplies one"
        );
        assert_eq!(fixture.store.vector_count, actual);
    }
}
