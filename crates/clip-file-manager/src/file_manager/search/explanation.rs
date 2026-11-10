//! Bounded evidence from matching segments; no extraction or inference during display.
use super::{retrieval::Hit, store::Store};
use crate::file_manager::filesystem::Entry;
use rusqlite::{OptionalExtension, params};
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(super) const MAX_PASSAGES: usize = 3;
#[derive(Clone, Copy)]
pub(super) struct SegmentMatch {
    pub id: i64,
    pub score: f64,
    pub keyword: bool,
}
pub(super) fn merge(target: &mut Vec<SegmentMatch>, incoming: &[SegmentMatch]) {
    target.extend_from_slice(incoming);
    target.sort_by(|a, b| {
        b.keyword
            .cmp(&a.keyword)
            .then(b.score.total_cmp(&a.score))
            .then(a.id.cmp(&b.id))
    });
    let mut seen = HashSet::new();
    target.retain(|item| seen.insert(item.id));
    target.truncate(MAX_PASSAGES);
}

#[derive(Clone, Default)]
pub(super) struct Explanation {
    pub unavailable: bool,
    pub coverage: String,
    pub passages: Vec<Passage>,
}
#[derive(Clone)]
pub(super) struct Passage {
    pub kind: String,
    pub location: String,
    pub text: String,
}
pub(super) type Explanations = HashMap<PathBuf, Explanation>;

pub(super) fn clipped(text: &str, count: usize) -> String {
    let mut chars = text.chars();
    let mut result = chars.by_ref().take(count).collect::<String>();
    if chars.next().is_some() {
        result.push('…');
    }
    result
}

/// A Unicode-safe context window around a literal case-insensitive match.
pub(super) fn excerpt(text: &str, needle: &str) -> String {
    let found = text
        .to_lowercase()
        .find(&needle.to_lowercase())
        .unwrap_or(0);
    let mut folded = 0;
    let mut original = 0;
    for (index, ch) in text.chars().enumerate() {
        let end = folded + ch.to_lowercase().map(char::len_utf8).sum::<usize>();
        if found < end {
            original = index;
            break;
        }
        folded = end;
    }
    let start = original.saturating_sub(64);
    let window = text.chars().skip(start).collect::<String>();
    format!(
        "{}{}",
        if start > 0 { "…" } else { "" },
        clipped(&window, 320)
    )
}

/// Fetch bounded cached passages for admitted files; the caller labels last-known rows.
pub(super) fn read(
    store: &Store,
    hits: &[Hit],
    entries: &[Entry],
    pipeline: &str,
    text: &str,
    cancel: &Arc<AtomicBool>,
) -> Result<Explanations, String> {
    let _cancellation = super::retrieval::SqlCancellation::new(&store.connection, cancel);
    let admitted = entries
        .iter()
        .map(|entry| &entry.path)
        .collect::<HashSet<_>>();
    let expression = super::retrieval::terms(text)?.join(" OR ");
    let keywords = keyword_snippets(store, hits, &admitted, &expression)?;
    let base = "FROM segments s JOIN files f ON f.id=s.file_id JOIN paths p ON p.file_id=f.id WHERE p.path=?1 AND s.id=?2 AND (?3='' OR f.pipeline=?3)";
    let mut segment = store.connection.prepare(&format!("SELECT s.location,substr(s.text,1,400),coalesce(f.coverage,'unknown'),s.modality {base}")).map_err(|error| error.to_string())?;
    let mut result = HashMap::new();
    for hit in hits.iter().filter(|hit| admitted.contains(&hit.entry.path)) {
        let mut explanation = Explanation::default();
        for found in hit.segments.iter().take(MAX_PASSAGES) {
            if cancel.load(Ordering::Relaxed) {
                return Err("Search cancelled".into());
            }
            let path = hit.entry.path.as_os_str().as_encoded_bytes();
            let row = |row: &rusqlite::Row<'_>| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            };
            let data = segment
                .query_row(params![path, found.id, pipeline], row)
                .optional()
                .map_err(|error| error.to_string())?;
            let Some((location, mut text, coverage, modality)) = data else {
                continue;
            };
            if found.keyword {
                let Some(snippet) = keywords.get(&found.id) else {
                    continue;
                };
                text.clone_from(snippet);
            }
            explanation.coverage = clipped(&coverage, 200);
            let location = clipped(&location, 160);
            if explanation
                .passages
                .iter()
                .any(|passage| passage.location == location)
            {
                continue;
            }
            explanation.passages.push(Passage {
                kind: if found.keyword {
                    "Keyword match".into()
                } else {
                    format!("Semantic {modality} match")
                },
                location,
                text: clipped(&text.split_whitespace().collect::<Vec<_>>().join(" "), 320),
            });
        }
        result.insert(hit.entry.path.clone(), explanation);
    }
    Ok(result)
}

fn keyword_snippets(
    store: &Store,
    hits: &[Hit],
    admitted: &HashSet<&PathBuf>,
    expression: &str,
) -> Result<HashMap<i64, String>, String> {
    let ids = hits
        .iter()
        .filter(|hit| admitted.contains(&hit.entry.path))
        .flat_map(|hit| hit.segments.iter().take(MAX_PASSAGES))
        .filter(|found| found.keyword)
        .map(|found| found.id)
        .collect::<BTreeSet<_>>();
    if ids.is_empty() || expression.is_empty() {
        return Ok(HashMap::new());
    }
    if ids.len() > super::retrieval::LIMIT * MAX_PASSAGES {
        return Err("Evidence request exceeds the result window".into());
    }
    let ids = serde_json::to_string(&ids).map_err(|error| error.to_string())?;
    // Adding zero deliberately prevents SQLite from choosing a separate FTS
    // rowid/MATCH lookup per ID. Walk the posting list once, generating snippets
    // only for the bounded requested IDs. The caller still validates each
    // segment's path and pipeline before using the cached snippet.
    let mut statement = store.connection.prepare(
        "SELECT rowid,snippet(segment_fts,0,'','',' … ',32) FROM segment_fts WHERE segment_fts MATCH ?1 AND rowid+0 IN (SELECT value FROM json_each(?2))"
    ).map_err(|error| error.to_string())?;
    statement
        .query_map(params![expression, ids], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|error| error.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_is_bounded_deduplicated_and_keeps_keyword_matches_first() {
        let mut matches = Vec::new();
        for id in 0..8 {
            merge(
                &mut matches,
                &[SegmentMatch {
                    id,
                    score: id as f64,
                    keyword: false,
                }],
            );
        }
        merge(
            &mut matches,
            &[
                SegmentMatch {
                    id: 1,
                    score: 0.1,
                    keyword: true,
                },
                SegmentMatch {
                    id: 7,
                    score: 0.2,
                    keyword: true,
                },
            ],
        );
        assert_eq!(
            matches.iter().map(|item| item.id).collect::<Vec<_>>(),
            [7, 1, 6]
        );
        assert!(matches[0].keyword && matches[1].keyword);
    }

    #[test]
    fn literal_excerpt_handles_unicode_case_expansion_and_long_context() {
        let text = format!(
            "{}İstanbul café MATCH end{}",
            "海".repeat(300),
            "尾".repeat(400)
        );
        let window = excerpt(&text, "match");
        assert!(window.starts_with('…'));
        assert!(window.contains("İstanbul café MATCH end"));
        assert!(window.ends_with('…'));
        assert_eq!(window.chars().count(), 322);
        assert_eq!(excerpt("İstanbul café", "CAFÉ"), "İstanbul café");
        assert_eq!(clipped("海🎵é", 2), "海🎵…");
        assert_eq!(clipped("海🎵", 2), "海🎵");
    }
}
