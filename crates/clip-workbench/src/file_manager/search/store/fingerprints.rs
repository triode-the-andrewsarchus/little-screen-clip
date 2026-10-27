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

pub(in crate::file_manager::search) fn format(entry: &Entry) -> Vec<u8> {
    // Readers dispatch on suffix. Equal bytes with different decoding routes
    // must not inherit another format's extraction or unsupported outcome.
    entry
        .path
        .extension()
        .unwrap_or_default()
        .as_encoded_bytes()
        .to_ascii_lowercase()
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
        segments.push(decode_segment(row)?);
    }
    if segments.is_empty() || segments.len() > 2048 {
        return Err("Invalid full-source reuse segment count".into());
    }
    Ok(segments)
}
fn decode_segment(row: &rusqlite::Row<'_>) -> Result<Segment, String> {
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
