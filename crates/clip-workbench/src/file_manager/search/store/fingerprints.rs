//! Optional derived cache: only a complete verified source digest can skip extraction.
use super::{EXTRACTION_VERSION, Segment, Store, decode_vector, record_storage_fault};
use crate::file_manager::filesystem::Entry;
use rusqlite::{OptionalExtension, params};

// This cache is recreated on open and deliberately excluded from backup imports.
// Losing it changes performance only; canonical segment hashes remain authoritative.
pub(super) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS source_digests(
    file_id INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    digest BLOB NOT NULL CHECK(length(digest)=32), format BLOB NOT NULL,
    pipeline TEXT NOT NULL, extraction_version INTEGER NOT NULL, coverage TEXT NOT NULL
); CREATE INDEX IF NOT EXISTS source_digest_lookup ON source_digests(digest,format,pipeline,extraction_version);";
pub(super) const PDF_SPARSE_POLICY: &[u8] = b"\0pdf-nonwhitespace-v2";
pub(super) const MEDIA_TIMELINE_POLICY: &[u8] = b"\0media-timeline-v2";

pub(in crate::file_manager::search) fn format(entry: &Entry) -> Vec<u8> {
    // Readers dispatch on suffix. Equal bytes with different decoding routes
    // must not inherit another format's extraction or unsupported outcome.
    let mut format = b"representation-v1\0".to_vec();
    // Extensionless README/Makefile routes differ from unknown bare names.
    format.push(u8::from(super::super::reader::supports_content(entry)));
    format.extend(
        entry
            .path
            .extension()
            .unwrap_or_default()
            .as_encoded_bytes()
            .to_ascii_lowercase(),
    );
    if crate::file_manager::text_strip::typescript_suffix(&entry.path) {
        // Older unclassified inventories must not reuse the opposite decoder's
        // full-source result or a cursor from its dispatch route.
        format.extend_from_slice(&[0, 1, u8::from(entry.typescript)]);
    }
    if little_clip_core::pdf::is_pdf_path(&entry.path) {
        // Revisit old PDF page choices without invalidating other decoders or
        // exact canonical vectors. Full-source reuse/checkpoints include this.
        format.extend_from_slice(PDF_SPARSE_POLICY);
    } else if little_clip_core::audio::is_audio_path(&entry.path) || entry.is_video() {
        format.extend_from_slice(MEDIA_TIMELINE_POLICY);
    }
    format
}
impl Store {
    pub fn remember_source(
        &self,
        file: i64,
        revision: u64,
        pipeline: &str,
        entry: &Entry,
        digest: &[u8; 32],
    ) -> Result<(), String> {
        self.connection.execute("INSERT OR REPLACE INTO source_digests(file_id,digest,format,pipeline,extraction_version,coverage)
            SELECT id,?4,?5,pipeline,extraction_version,coverage FROM files
            WHERE id=?1 AND revision=?2 AND pipeline=?3 AND state='ready' AND coverage IN ('complete','sampled: visual frames; selected audio track','sampled: visual frames; no audio track observed')",
            params![file, revision as i64, pipeline, digest.as_slice(), format(entry)])
            .map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        Ok(())
    }
    pub fn reusable_source(
        &self,
        entry: &Entry,
        pipeline: &str,
        digest: &[u8; 32],
    ) -> Result<Option<(Vec<Segment>, String)>, String> {
        let cached: Option<(i64, String)> = self.connection.query_row(
            "SELECT file_id,coverage FROM source_digests WHERE digest=?1 AND format=?2 AND pipeline=?3 AND extraction_version=?4 LIMIT 1",
            params![digest.as_slice(), format(entry), pipeline, EXTRACTION_VERSION], |row| Ok((row.get(0)?,row.get(1)?)))
            .optional().map_err(|error| record_storage_fault(&self.storage_fault, error))?;
        let Some((file, coverage)) = cached else {
            return Ok(None);
        };
        let segments = read_segments(&self.connection, file, pipeline)?;
        Ok(Some((segments, coverage)))
    }
}

pub(super) fn read_segments(
    connection: &rusqlite::Connection,
    file: i64,
    pipeline: &str,
) -> Result<Vec<Segment>, String> {
    let mut statement = connection.prepare("SELECT modality,location,text,input_hash,vector FROM segments WHERE file_id=?1 AND pipeline=?2 ORDER BY id LIMIT 2049").map_err(|e| e.to_string())?;
    let mut rows = statement
        .query(params![file, pipeline])
        .map_err(|e| e.to_string())?;
    let mut segments = Vec::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        if segments.len() == 2048 {
            return Err("Invalid full-source reuse segment count".into());
        }
        segments.push(decode_segment(row)?);
    }
    if segments.is_empty() {
        return Err("Invalid full-source reuse segment count".into());
    }
    Ok(segments)
}
fn decode_segment(row: &rusqlite::Row<'_>) -> Result<Segment, String> {
    // Validate borrowed SQLite cells before allocating owned strings/vectors.
    // Together with the segment count this keeps retained reuse below 32 MiB.
    for (index, limit) in [(0, 16), (1, 4096), (2, 8192), (3, 32), (4, 1536)] {
        let bytes = match row.get_ref(index).map_err(|error| error.to_string())? {
            rusqlite::types::ValueRef::Text(bytes) | rusqlite::types::ValueRef::Blob(bytes) => {
                bytes
            }
            rusqlite::types::ValueRef::Null if index == 4 => continue,
            _ => return Err("Invalid full-source reuse field".into()),
        };
        if bytes.len() > limit {
            return Err("Full-source reuse segment exceeds its staging budget".into());
        }
    }
    let vector: Option<Vec<u8>> = row.get(4).map_err(|e| e.to_string())?;
    let segment = Segment {
        modality: row.get(0).map_err(|e| e.to_string())?,
        location: row.get(1).map_err(|e| e.to_string())?,
        text: row.get(2).map_err(|e| e.to_string())?,
        input_hash: row.get(3).map_err(|e| e.to_string())?,
        vector: vector.map(|bytes| decode_vector(&bytes)).transpose()?,
    };
    if segment.text.len() > 8192 || segment.input_hash.len() != 32 {
        return Err("Invalid full-source reuse segment".into());
    }
    Ok(segment)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_page_policy_revisits_only_pdf_representation_and_keeps_canonical_vectors() {
        let mut fixture = super::super::TestFixture::new();
        let directory = fixture.entry.path.parent().unwrap();
        let path = directory.join("alias.pdf");
        std::fs::hard_link(&fixture.entry.path, &path).unwrap();
        let root = crate::file_manager::search::reader::SourceRoot::from_root(&fixture.root);
        // Capture both aliases after link creation, which changes ctime.
        let entries = crate::file_manager::filesystem::read_directory(
            directory,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap()
        .entries;
        let pdf = entries.iter().find(|entry| entry.path == path).unwrap();
        let text = entries
            .iter()
            .find(|entry| entry.path == fixture.entry.path)
            .unwrap();
        let root_id = fixture.store.enroll_source(&root).unwrap();
        let generation = fixture.store.begin_scan(root_id).unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        let (old, _) = fixture
            .store
            .observe(root_id, generation, pdf, &metadata, "model")
            .unwrap();
        let segments = read_segments(&fixture.store.connection, fixture.file, "model").unwrap();
        fixture
            .store
            .publish(old, pdf.revision, "model", &segments, "complete")
            .unwrap();
        fixture
            .store
            .remember_source(old, pdf.revision, "model", pdf, &[9; 32])
            .unwrap();
        let legacy = format(pdf)
            .strip_suffix(PDF_SPARSE_POLICY)
            .unwrap()
            .to_vec();
        fixture
            .store
            .connection
            .execute(
                "UPDATE files SET format=?1 WHERE id=?2",
                params![legacy, old],
            )
            .unwrap();
        fixture
            .store
            .connection
            .execute(
                "UPDATE source_digests SET format=?1 WHERE file_id=?2",
                params![legacy, old],
            )
            .unwrap();
        assert!(
            fixture
                .store
                .reusable_source(pdf, "model", &[9; 32])
                .unwrap()
                .is_none()
        );
        let (current, pending) = fixture
            .store
            .observe(root_id, generation, pdf, &metadata, "model")
            .unwrap();
        assert_ne!(current, old);
        assert!(pending);
        assert!(
            fixture
                .store
                .reusable_vector("model", "text", &[1; 32])
                .unwrap()
                .is_some()
        );
        assert_eq!(
            read_segments(&fixture.store.connection, old, "model").unwrap()[0].text,
            "original content"
        );
        let (text_id, _) = fixture
            .store
            .observe(root_id, generation, text, &metadata, "model")
            .unwrap();
        fixture
            .store
            .publish(text_id, text.revision, "model", &segments, "complete")
            .unwrap();
        let (unchanged, pending) = fixture
            .store
            .observe(root_id, generation, text, &metadata, "model")
            .unwrap();
        assert_eq!(unchanged, fixture.file);
        assert!(!pending);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original content");
    }

    #[test]
    fn restored_segment_strings_are_bounded_before_owned_reuse_allocation() {
        let fixture = super::super::TestFixture::new();
        for (column, value) in [
            ("modality", rusqlite::types::Value::Text("x".repeat(17))),
            ("location", rusqlite::types::Value::Text("x".repeat(4097))),
            ("text", rusqlite::types::Value::Text("x".repeat(8193))),
            ("input_hash", rusqlite::types::Value::Blob(vec![1; 33])),
        ] {
            fixture.store.connection.execute(
                "UPDATE segments SET modality='text',location=?1,text=?2,input_hash=?3,vector=NULL",
                params!["x".repeat(4096), "x".repeat(8192), vec![1_u8; 32]],
            ).unwrap();
            assert!(read_segments(&fixture.store.connection, fixture.file, "model").is_ok());
            fixture
                .store
                .connection
                .execute(&format!("UPDATE segments SET {column}=?1"), [value])
                .unwrap();
            let error = read_segments(&fixture.store.connection, fixture.file, "model")
                .err()
                .unwrap();
            assert!(error.contains("staging budget"), "{column}: {error}");
        }
        assert_eq!(
            std::fs::read_to_string(&fixture.entry.path).unwrap(),
            "original content"
        );
    }
}
