//! Literal reuse is gated by fresh source observations, never by embedding similarity.
use super::{EXTRACTION_VERSION, Store};
use crate::file_manager::{
    filesystem::Entry,
    search::{
        content::{self, Literal},
        policy,
        reader::SourceRoot,
        retrieval::SqlCancellation,
        session::SourceSession,
    },
};
use rusqlite::{OptionalExtension, params};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const MAX_BYTES: i64 = 16 * 1024 * 1024;
const MAX_PARTS: i64 = 2048;

struct Publication {
    file: i64,
    coverage: String,
    parts: i64,
    bytes: i64,
    media: bool,
    incompatible: bool,
    checkpoint: bool,
}

struct TextPart {
    location: String,
    text: String,
}

// Model passages overlap. Reassemble their original UTF-8 ranges before
// applying literal whitespace normalization, or overlap would invent phrases.
fn append(parts: &mut Vec<TextPart>, location: &str, text: &str) -> Option<()> {
    let (location, range) = location.rsplit_once(" · UTF-8 bytes ")?;
    let (start, end) = range.split_once("..")?;
    let (start, end) = (start.parse::<usize>().ok()?, end.parse::<usize>().ok()?);
    if end.checked_sub(start)? != text.len() || text.is_empty() {
        return None;
    }
    if parts.last().is_none_or(|part| part.location != location) {
        if start != 0 || parts.iter().any(|part| part.location == location) {
            return None;
        }
        parts.push(TextPart {
            location: location.into(),
            text: String::new(),
        });
    }
    parts.last_mut()?.append(start, text)
}

impl TextPart {
    fn append(&mut self, start: usize, text: &str) -> Option<()> {
        let overlap = self.text.len().checked_sub(start)?.min(text.len());
        if self.text.get(start..start + overlap)? != text.get(..overlap)? {
            return None;
        }
        self.text.try_reserve(text.len() - overlap).ok()?;
        self.text.push_str(text.get(overlap..)?);
        Some(())
    }
}

impl Store {
    pub(in crate::file_manager::search) fn cached_content(
        &self,
        root: &SourceRoot,
        entry: &Entry,
        needle: &str,
        cancel: &Arc<AtomicBool>,
    ) -> Result<Option<content::Match>, String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Search cancelled".into());
        }
        let Some(session) = root
            .session
            .as_ref()
            .filter(|session| session.unique_on(root.mount))
        else {
            return Ok(None);
        };
        let Some(identity) = entry.identity else {
            return Ok(None);
        };
        if !reusable_entry(root, entry) {
            return Ok(None);
        }
        let _cancellation = SqlCancellation::new(&self.connection, cancel);
        let epoch = self.cache_epoch()?;
        let candidate = self.content_publication(root, entry, session, identity.object())?;
        let Some(publication) = candidate else {
            return Ok(None);
        };
        if !(1..=MAX_PARTS).contains(&publication.parts)
            || !(1..=MAX_BYTES).contains(&publication.bytes)
            || publication.incompatible
        {
            return Ok(None);
        }
        let partial =
            publication.coverage != "complete" || publication.media || publication.checkpoint;
        let Some(parts) = self.content_parts(publication.file, cancel)? else {
            return Ok(None);
        };
        let result = match_parts(parts, &publication.coverage, partial, needle);
        self.finish_vector_cache(epoch)?;
        Ok(result)
    }

    fn content_publication(
        &self,
        root: &SourceRoot,
        entry: &Entry,
        session: &SourceSession,
        identity: (u64, u64),
    ) -> Result<Option<Publication>, String> {
        let observation = serde_json::to_string(session).map_err(|error| error.to_string())?;
        let scope = self.source_scope(root, Some(&observation));
        let mut statement = self.connection.prepare_cached(
            "SELECT f.id,substr(coalesce(f.coverage,''),1,3072),count(s.id),sum(length(CAST(s.text AS BLOB))),max(s.modality<>'text'),max(s.pipeline<>f.pipeline),EXISTS(SELECT 1 FROM file_checkpoints c WHERE c.file_id=f.id) FROM paths p JOIN files f ON f.id=p.file_id JOIN segments s ON s.file_id=f.id WHERE p.path=?1 AND f.device=?2 AND f.inode=?3 AND f.revision=?4 AND f.scope=?5 AND f.extraction_version=?6 AND f.format=?9 AND f.state='ready' AND coalesce(json_extract(f.metadata,'$.typescript'),0)=?8 AND EXISTS(SELECT 1 FROM memberships m JOIN roots r ON r.id=m.root_id JOIN source_sessions ss ON ss.root_id=r.id WHERE m.path=p.path AND m.verified=1 AND r.available=1 AND ss.continuity=1 AND ss.scope=f.scope AND ss.observation=?7) GROUP BY f.id"
        ).map_err(|error| error.to_string())?;
        statement
            .query_row(
                params![
                    entry.path.as_os_str().as_encoded_bytes(),
                    identity.0 as i64,
                    identity.1 as i64,
                    entry.revision as i64,
                    scope,
                    EXTRACTION_VERSION,
                    observation,
                    entry.typescript,
                    super::source_format(entry)
                ],
                |row| {
                    Ok(Publication {
                        file: row.get(0)?,
                        coverage: row.get(1)?,
                        parts: row.get(2)?,
                        bytes: row.get(3)?,
                        media: row.get(4)?,
                        incompatible: row.get(5)?,
                        checkpoint: row.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    fn content_parts(
        &self,
        file: i64,
        cancel: &AtomicBool,
    ) -> Result<Option<Vec<TextPart>>, String> {
        let mut statement = self.connection.prepare_cached(
            "SELECT substr(location,1,4096),text FROM segments WHERE file_id=?1 AND modality='text' ORDER BY id"
        ).map_err(|error| error.to_string())?;
        let mut rows = statement.query([file]).map_err(|error| error.to_string())?;
        let mut parts = Vec::new();
        while let Some(row) = rows.next().map_err(|error| error.to_string())? {
            if cancel.load(Ordering::Relaxed) {
                return Err("Search cancelled".into());
            }
            let location: String = row.get(0).map_err(|error| error.to_string())?;
            let text: String = row.get(1).map_err(|error| error.to_string())?;
            if append(&mut parts, &location, &text).is_none() {
                return Ok(None);
            }
        }
        Ok(Some(parts))
    }
}

fn match_parts(
    parts: Vec<TextPart>,
    coverage: &str,
    partial: bool,
    needle: &str,
) -> Option<content::Match> {
    let coverage = if coverage.is_empty() {
        "unknown coverage"
    } else if partial && coverage == "complete" {
        "partial: indexed text only; non-text or unfinished content not examined"
    } else {
        coverage
    };
    let mut literal = Literal::new(needle);
    for part in parts {
        if let Some(explanation) = literal.push(
            &part.text,
            &part.location,
            format!("{coverage}; current indexed text reused"),
        ) {
            return Some((true, partial, Some(explanation)));
        }
    }
    // An incomplete index cannot prove a negative: fresh extraction may
    // reach text not represented by its current passages.
    (!partial).then_some((false, false, None))
}

// The walker supplies a fresh, root-validated inventory. Retain its ordinary
// content policy here as well before consulting derived text.
fn reusable_entry(root: &SourceRoot, entry: &Entry) -> bool {
    entry.regular_file
        && !entry.symlink
        && entry.system.is_none()
        && entry.warning.is_none()
        && entry
            .identity
            .is_some_and(|identity| identity.object().0 == root.device)
        && entry.path.starts_with(&root.path)
        && policy::excluded(&entry.path).is_none()
        && !policy::secret_file(&entry.path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_manager::search::store::{Segment, TestFixture};

    fn publish(fixture: &mut TestFixture, coverage: &str) {
        let segments = [(0, 12, "original con"), (9, 16, "content")]
            .into_iter()
            .map(|(start, end, text)| Segment {
                modality: "text".into(),
                location: format!("document text · UTF-8 bytes {start}..{end}"),
                text: text.into(),
                input_hash: vec![start as u8; 32],
                vector: None,
            })
            .collect::<Vec<_>>();
        assert!(
            fixture
                .store
                .publish(
                    fixture.file,
                    fixture.entry.revision,
                    "model",
                    &segments,
                    coverage
                )
                .unwrap()
        );
    }

    fn cached(fixture: &TestFixture, source: &SourceRoot, needle: &str) -> Option<content::Match> {
        fixture
            .store
            .cached_content(
                source,
                &fixture.entry,
                needle,
                &Arc::new(AtomicBool::new(false)),
            )
            .unwrap()
    }

    #[test]
    fn overlapping_passages_reconstruct_exact_text_for_positive_and_negative_matches() {
        let mut fixture = TestFixture::new();
        publish(&mut fixture, "complete");
        let source = SourceRoot::from_root(&fixture.root);
        let result = cached(&fixture, &source, "ORIGINAL  content");
        if !source
            .session
            .as_ref()
            .is_some_and(|session| session.unique())
        {
            assert!(result.is_none());
            return;
        }
        let (matched, partial, evidence) = result.expect("trusted indexed text should be reused");
        assert!(matched);
        assert!(!partial);
        assert!(evidence.unwrap().coverage.contains("indexed text reused"));
        for absent in ["con content", "not present"] {
            let result = cached(&fixture, &source, absent).unwrap();
            assert!(!result.0 && !result.1 && result.2.is_none());
        }
    }

    #[test]
    fn incomplete_or_visual_coverage_can_prove_a_match_but_never_a_negative() {
        let mut fixture = TestFixture::new();
        let source = SourceRoot::from_root(&fixture.root);
        for alteration in [
            "UPDATE files SET coverage='partial: representation budget'",
            "UPDATE segments SET modality='image' WHERE text='content'",
            "INSERT INTO file_checkpoints SELECT id,revision,pipeline,extraction_version,'{}',2 FROM files",
        ] {
            publish(&mut fixture, "complete");
            fixture.store.connection.execute_batch(alteration).unwrap();
            if let Some((matched, partial, evidence)) = cached(&fixture, &source, "original") {
                assert!(matched && partial);
                assert!(evidence.unwrap().coverage.starts_with("partial:"));
            } else {
                assert!(
                    !source
                        .session
                        .as_ref()
                        .is_some_and(|session| session.unique())
                );
            }
            assert!(cached(&fixture, &source, "not present").is_none());
        }
    }

    #[test]
    fn stale_untrusted_malformed_or_oversized_index_entries_are_cache_misses() {
        for alteration in [
            "UPDATE files SET state='pending'",
            "UPDATE files SET revision=revision+1",
            "UPDATE files SET inode=inode+1",
            "UPDATE files SET extraction_version=0",
            "UPDATE files SET metadata=json_set(metadata,'$.typescript',1)",
            "UPDATE segments SET pipeline='different'",
            "UPDATE memberships SET verified=0",
            "UPDATE roots SET available=0",
            "UPDATE source_sessions SET continuity=0",
            "UPDATE source_sessions SET observation='{}'",
            "UPDATE source_sessions SET scope='different'",
            "UPDATE segments SET location='unstructured'",
            "UPDATE segments SET location='document text · UTF-8 bytes 13..20' WHERE text='content'",
            "UPDATE segments SET text='CONTENT' WHERE text='content'",
            "UPDATE segments SET text=CAST(zeroblob(16777217) AS TEXT) WHERE text='content'",
        ] {
            let mut fixture = TestFixture::new();
            publish(&mut fixture, "complete");
            fixture.store.connection.execute_batch(alteration).unwrap();
            assert!(
                cached(&fixture, &SourceRoot::from_root(&fixture.root), "original").is_none(),
                "{alteration}"
            );
        }
    }

    #[test]
    fn changed_or_unknown_live_identity_never_reuses_old_passages() {
        let mut fixture = TestFixture::new();
        publish(&mut fixture, "complete");
        let source = SourceRoot::from_root(&fixture.root);
        let mut changed = source.clone();
        changed.mount = source.mount.map(|mount| mount + 1);
        assert!(cached(&fixture, &changed, "original").is_none());
        changed = source.clone();
        changed.session = None;
        assert!(cached(&fixture, &changed, "original").is_none());
        for field in ["boot", "namespace", "mount", "unique_mount"] {
            let Some(session) = source.session.as_ref() else {
                continue;
            };
            let mut observation = serde_json::to_value(session).unwrap();
            observation[field] = match field {
                "boot" => serde_json::json!(uuid::Uuid::new_v4()),
                "unique_mount" => serde_json::Value::Null,
                _ => serde_json::json!(observation[field].as_u64().unwrap() + 1),
            };
            changed = source.clone();
            changed.session = Some(serde_json::from_value(observation).unwrap());
            assert!(cached(&fixture, &changed, "original").is_none(), "{field}");
        }
        fixture.entry.revision = fixture.entry.revision.wrapping_add(1);
        assert!(cached(&fixture, &source, "original").is_none());
        fixture.entry.identity = None;
        assert!(cached(&fixture, &source, "original").is_none());
    }

    #[test]
    fn reconstruction_rejects_utf8_splits_gaps_and_repeated_groups() {
        let mut parts = Vec::new();
        assert!(append(&mut parts, "page 1 · UTF-8 bytes 0..5", "a猫b").is_some());
        assert!(append(&mut parts, "page 1 · UTF-8 bytes 2..5", "xyz").is_none());
        assert!(append(&mut parts, "page 1 · UTF-8 bytes 6..7", "c").is_none());
        assert!(append(&mut parts, "page 1 · UTF-8 bytes 4..6", "bc").is_some());
        assert_eq!(parts[0].text, "a猫bc");
        assert!(append(&mut parts, "page 2 · UTF-8 bytes 0..1", "d").is_some());
        assert!(append(&mut parts, "page 1 · UTF-8 bytes 0..1", "a").is_none());
    }

    #[test]
    fn cancelled_reuse_returns_no_answer_and_does_not_poison_later_queries() {
        let mut fixture = TestFixture::new();
        publish(&mut fixture, "complete");
        let source = SourceRoot::from_root(&fixture.root);
        let result = fixture.store.cached_content(
            &source,
            &fixture.entry,
            "original",
            &Arc::new(AtomicBool::new(true)),
        );
        assert!(matches!(result, Err(error) if error.contains("cancelled")));
        let result = cached(&fixture, &source, "original");
        assert_eq!(
            result.is_some(),
            source
                .session
                .as_ref()
                .is_some_and(|session| session.unique())
        );
    }
}
