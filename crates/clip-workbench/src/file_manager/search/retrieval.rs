//! Hard-filtered lexical and per-modality retrieval with bounded rank fusion.
#[cfg(test)]
mod benchmark;
#[cfg(test)]
mod quality;
#[cfg(test)]
use super::store::cosine_score;
use super::{
    explanation::{self, SegmentMatch},
    query::{Query, Scope},
    store::{CacheEpoch, DIMENSION, LexicalNames, Store},
};
use crate::file_manager::{
    filesystem::{self, Entry},
    sorting::natural_cmp,
};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering},
    },
};

pub(super) const LIMIT: usize = 500;
// Reject nonmatching names before file/session lookups. Starting at paths also
// retains the planner's parent/range indexes for narrow directory scopes.
const NAMES_SELECT: &str = "SELECT p.path,f.id FROM paths p CROSS JOIN files f INDEXED BY file_search ON f.id=p.file_id WHERE clip_filename_matches(p.path,:folded) AND ";
// Keep paths before file admission so the exact ranking threshold can reject
// dominated postings before a file-index lookup. CROSS JOIN fixes only join
// order: every surviving row still passes current() and candidate() admission.
const FTS_SELECT: &str = "SELECT p.path,f.id,bm25(segment_fts),s.id FROM segment_fts CROSS JOIN segments s INDEXED BY segment_identity ON s.id=segment_fts.rowid CROSS JOIN paths p ON p.file_id=s.file_id CROSS JOIN files f INDEXED BY file_search ON f.id=s.file_id WHERE segment_fts MATCH :expression AND clip_lexical_might_rank(p.path,bm25(segment_fts)) AND ";
const SEMANTIC_SELECT: &str = "SELECT p.path,f.id,s.modality,CASE WHEN clip_vector_cached(s.id) THEN 1 ELSE s.vector END,s.id FROM paths p JOIN files f ON f.id=p.file_id JOIN segments s INDEXED BY segment_admission ON s.file_id=f.id WHERE f.state='ready' AND ";
// Published unsupported files have no segments; skip their file admission.
// Global indexed searches cannot use a directory/path-range restriction. Admit
// each file/path membership before visiting its segments, rather than repeating
// the same membership query for every segment. Scoped searches keep the planner's
// ability to start at path_parent or the path range index.
const INDEXED_SEMANTIC_SELECT: &str = "SELECT p.path,f.id,s.modality,CASE WHEN clip_vector_cached(s.id) THEN 1 ELSE s.vector END,s.id FROM files f INDEXED BY file_search CROSS JOIN paths p ON p.file_id=f.id CROSS JOIN segments s INDEXED BY segment_admission ON s.file_id=f.id WHERE f.state='ready' AND ";

#[derive(Clone)]
struct Ranked {
    entry: Arc<Entry>,
    tier: u8,
    score: f64,
    segments: Vec<SegmentMatch>,
}
pub(super) struct Hit {
    pub entry: Entry,
    pub segments: Vec<SegmentMatch>,
}
struct SemanticScore {
    value: f32,
    // Metadata constraints may require parsing before the vector is scored.
    entry: Option<Arc<Entry>>,
}
struct RankKey<'a> {
    tier: u8,
    score: f64,
    name: &'a str,
    path: &'a Path,
}
impl RankKey<'_> {
    fn compare(&self, other: &Self) -> Ordering {
        self.compare_name(other.tier, other.score, other.name)
            .then_with(|| other.path.cmp(self.path))
    }
    fn compare_name(&self, tier: u8, score: f64, name: &str) -> Ordering {
        tier.cmp(&self.tier)
            .then(self.score.total_cmp(&score))
            .then_with(|| natural_cmp(name, self.name))
    }
}
impl Ranked {
    fn key(&self) -> RankKey<'_> {
        RankKey {
            tier: self.tier,
            score: self.score,
            name: &self.entry.name,
            path: &self.entry.path,
        }
    }
}
impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key().compare(&other.key())
    }
}
impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl PartialEq for Ranked {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Ranked {}

#[derive(Default)]
struct Top {
    by_path: HashMap<PathBuf, Ranked>,
    by_name: Option<HashMap<String, usize>>,
    ordered: BTreeSet<Ranked>,
}
impl Top {
    fn accepts(&self, path: &Path, name: &str, tier: u8, score: f64) -> bool {
        self.by_path.contains_key(path)
            || self.ordered.len() < LIMIT
            || RankKey {
                path,
                name,
                tier,
                score,
            }
            .compare(&self.ordered.first().unwrap().key())
            .is_gt()
    }
    // Coarse hints cannot resolve equal basenames in different directories.
    // Retain every such tie and every name with a currently retained path.
    fn accepts_name(&self, name: &str, score: f64) -> bool {
        self.by_name
            .as_ref()
            .is_none_or(|names| names.contains_key(name))
            || self.ordered.len() < LIMIT
            || self
                .ordered
                .first()
                .unwrap()
                .key()
                .compare_name(3, score, name)
                .is_le()
    }
    fn remove(&mut self, path: &Path) -> Option<Ranked> {
        let old = self.by_path.remove(path)?;
        self.ordered.remove(&old);
        if let Some(names) = self.by_name.as_mut()
            && let Some(count) = names.get_mut(&old.entry.name)
        {
            *count -= 1;
            if *count == 0 {
                names.remove(&old.entry.name);
            }
        }
        Some(old)
    }
    fn offer(&mut self, mut candidate: Ranked) {
        if !self.accepts(
            &candidate.entry.path,
            &candidate.entry.name,
            candidate.tier,
            candidate.score,
        ) {
            return;
        }
        if let Some(old) = self.remove(&candidate.entry.path) {
            let mut segments = candidate.segments.clone();
            explanation::merge(&mut segments, &old.segments);
            if candidate <= old {
                candidate = old;
            }
            candidate.segments = segments;
        }
        if let Some(names) = self.by_name.as_mut() {
            *names.entry(candidate.entry.name.clone()).or_default() += 1;
        }
        self.by_path
            .insert(candidate.entry.path.clone(), candidate.clone());
        self.ordered.insert(candidate);
        if self.ordered.len() > LIMIT {
            let worst = self.ordered.pop_first().unwrap();
            self.remove(&worst.entry.path);
        }
    }
    fn best(&self) -> impl Iterator<Item = &Ranked> {
        self.ordered.iter().rev()
    }
}

// The threshold contains only rows already admitted by this lexical request.
// Reuse Top's exact comparison; this rejects dominated rows, never authorizes
// a source or caps an unfiltered posting list. Retained paths still pass so
// their additional matching locations can be merged.
struct LexicalRankFilter<'a> {
    connection: &'a rusqlite::Connection,
    output: &'a mut Top,
    top: Arc<Mutex<Top>>,
}
impl<'a> LexicalRankFilter<'a> {
    #[cfg(test)]
    fn new(connection: &'a rusqlite::Connection, output: &'a mut Top) -> Result<Self, String> {
        Self::with_paths(connection, output, None)
    }
    fn with_paths(
        connection: &'a rusqlite::Connection,
        output: &'a mut Top,
        paths: Option<LexicalNames>,
    ) -> Result<Self, String> {
        let mut initial = std::mem::take(output);
        // Semantic Top sets and ordinary lexical scans need no name counters.
        initial.by_name = paths.as_ref().map(|_| {
            let mut names = HashMap::new();
            for item in initial.by_path.values() {
                *names.entry(item.entry.name.clone()).or_default() += 1;
            }
            names
        });
        let filter = Self {
            connection,
            top: Arc::new(Mutex::new(initial)),
            output,
        };
        let top = Arc::clone(&filter.top);
        connection
            .create_scalar_function(
                "clip_lexical_might_rank",
                2,
                rusqlite::functions::FunctionFlags::SQLITE_UTF8
                    | rusqlite::functions::FunctionFlags::SQLITE_DIRECTONLY,
                move |context| Self::allows(&top, context_path(context)?, -context.get::<f64>(1)?),
            )
            .map_err(|error| error.to_string())?;
        if let Some(paths) = paths {
            let top = Arc::clone(&filter.top);
            connection
                .create_scalar_function(
                    "clip_segment_might_rank",
                    2,
                    rusqlite::functions::FunctionFlags::SQLITE_UTF8
                        | rusqlite::functions::FunctionFlags::SQLITE_DIRECTONLY,
                    move |context| {
                        let id: i64 = context.get(0)?;
                        let Some(name) = paths.get(&id) else {
                            return Ok(true);
                        };
                        let top = top.lock().map_err(|_| {
                            rusqlite::Error::UserFunctionError(Box::new(std::io::Error::other(
                                "Lexical ranking unavailable",
                            )))
                        })?;
                        Ok(top.accepts_name(name, -context.get::<f64>(1)?))
                    },
                )
                .map_err(|error| error.to_string())?;
        }
        Ok(filter)
    }
    fn allows(top: &Mutex<Top>, path: &Path, score: f64) -> rusqlite::Result<bool> {
        let name = filesystem::display_path_cow(Path::new(path.file_name().unwrap_or_default()));
        let top = top.lock().map_err(|_| {
            rusqlite::Error::UserFunctionError(Box::new(std::io::Error::other(
                "Lexical ranking unavailable",
            )))
        })?;
        Ok(top.accepts(path, &name, 3, score))
    }
}
impl Drop for LexicalRankFilter<'_> {
    fn drop(&mut self) {
        let _ = self
            .connection
            .remove_function("clip_segment_might_rank", 2);
        let _ = self
            .connection
            .remove_function("clip_lexical_might_rank", 2);
        if let Ok(mut top) = self.top.lock() {
            *self.output = std::mem::take(&mut *top);
        }
    }
}

fn context_path<'a>(context: &'a rusqlite::functions::Context<'_>) -> rusqlite::Result<&'a Path> {
    use std::os::unix::ffi::OsStrExt;
    match context.get_raw(0) {
        rusqlite::types::ValueRef::Blob(bytes) => Ok(Path::new(std::ffi::OsStr::from_bytes(bytes))),
        value => Err(rusqlite::Error::InvalidFunctionParameterType(
            0,
            value.data_type(),
        )),
    }
}

// The coordinator exclusively owns its connection. Keep the callback scoped to
// this read request, including prepare/step, and remove it before later writes.
pub(super) struct SqlCancellation<'a>(&'a rusqlite::Connection);
impl<'a> SqlCancellation<'a> {
    pub(super) fn new(connection: &'a rusqlite::Connection, cancel: &Arc<AtomicBool>) -> Self {
        let cancel = Arc::clone(cancel);
        connection.progress_handler(4096, Some(move || cancel.load(AtomicOrdering::Relaxed)));
        Self(connection)
    }
}
impl Drop for SqlCancellation<'_> {
    fn drop(&mut self) {
        self.0.progress_handler(0, None::<fn() -> bool>);
    }
}

pub(super) struct Retrieval<'a> {
    pub store: &'a Store,
    pub pipeline: &'a str,
    pub vector: &'a [f32],
    pub path: &'a Path,
    pub query: &'a Query,
    pub text: &'a str,
    pub hidden: bool,
    pub index_hidden: bool,
    pub show_offline: bool,
    pub exclusions: &'a [PathBuf],
    pub floors: &'a BTreeMap<String, f32>,
    pub cancel: &'a Arc<AtomicBool>,
    pub progress: Option<&'a AtomicU64>,
}
impl Retrieval<'_> {
    pub fn search(&self) -> Result<Vec<Hit>, String> {
        check(self.cancel)?;
        let _cancellation = SqlCancellation::new(&self.store.connection, self.cancel);
        if self
            .floors
            .values()
            .any(|floor| !floor.is_finite() || !(-1.0..=1.0).contains(floor))
        {
            return Err("Semantic cosine floors must be finite values between -1 and 1".into());
        }
        validate_query_vector(self.vector)?;
        register_name_filter(&self.store.connection)?;
        let cache_epoch = if self.vector.is_empty() {
            None
        } else {
            Some(self.store.prepare_vector_cache()?)
        };
        let mut lexical = Top::default();
        let mut partial = false;
        self.lexical(None, &mut lexical)?;
        let terms = terms(self.text)?;
        if !terms.is_empty() {
            self.lexical(Some(&terms.join(" AND ")), &mut lexical)?;
            // A long natural-language description must not let one incidental
            // word outweigh a strong cross-modal match. Short keyword queries
            // retain broad recall (e.g. "window invoice" or "2019 tax document").
            if lexical.by_path.is_empty() && (self.vector.is_empty() || terms.len() <= 3) {
                self.lexical(Some(&terms.join(" OR ")), &mut lexical)?;
                partial = true;
            }
        }
        let semantics = if self.vector.is_empty() {
            BTreeMap::new()
        } else {
            if self.pipeline.is_empty() {
                return Err("Semantic scoring requires an identified model pipeline".into());
            }
            self.semantic()?
        };
        let result = fuse(&lexical, &semantics, self.text, partial);
        if let Some(epoch) = cache_epoch {
            self.store.finish_vector_cache(epoch)?;
        }
        Ok(result)
    }
}

// Apply the same escaped, Unicode-aware matcher before expensive membership
// lookups. This only rejects names; every surviving candidate still passes the
// complete source/session, visibility and metadata admission below.
fn register_name_filter(connection: &rusqlite::Connection) -> Result<(), String> {
    use rusqlite::functions::FunctionFlags;
    use rusqlite::types::ValueRef;
    connection
        .create_scalar_function(
            "clip_filename_matches",
            2,
            FunctionFlags::SQLITE_UTF8
                | FunctionFlags::SQLITE_DETERMINISTIC
                | FunctionFlags::SQLITE_DIRECTONLY,
            |context| {
                let path = context_path(context)?;
                let query = match context.get_raw(1) {
                    ValueRef::Text(bytes) => std::str::from_utf8(bytes)
                        .map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))?,
                    value => {
                        return Err(rusqlite::Error::InvalidFunctionParameterType(
                            1,
                            value.data_type(),
                        ));
                    }
                };
                let name =
                    filesystem::display_path_cow(Path::new(path.file_name().unwrap_or_default()));
                Ok(crate::file_manager::type_navigation::name_rank(&name, query).is_some())
            },
        )
        .map_err(|e| e.to_string())
}

pub(super) fn terms(text: &str) -> Result<Vec<String>, String> {
    let pattern = regex::Regex::new(r"[\p{L}\p{N}\p{M}\p{Co}]+").unwrap();
    let tokens = pattern
        .find_iter(text)
        .map(|m| m.as_str())
        .collect::<Vec<_>>();
    if tokens.len() > 128 {
        return Err("Semantic descriptions may contain at most 128 lexical terms".into());
    }
    Ok(tokens
        .iter()
        .enumerate()
        .map(|(i, token)| {
            format!(
                "\"{}\"{}",
                token.replace('"', "\"\""),
                if i + 1 == tokens.len() && token.chars().count() >= 3 {
                    "*"
                } else {
                    ""
                }
            )
        })
        .collect())
}

fn validate_query_vector(vector: &[f32]) -> Result<(), String> {
    if vector.is_empty() {
        return Ok(());
    }
    let norm_squared = vector
        .iter()
        .map(|&value| f64::from(value).powi(2))
        .sum::<f64>();
    if vector.len() != DIMENSION || !norm_squared.is_finite() || norm_squared < 1e-24 {
        return Err("Semantic query needs a finite nonzero 768-dimensional vector".into());
    }
    Ok(())
}

fn check(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(AtomicOrdering::Relaxed) {
        Err("Search cancelled".into())
    } else {
        Ok(())
    }
}
impl Retrieval<'_> {
    fn current(&self) -> String {
        // Semantic scoring requires an inventory-verified membership. The
        // model-independent lexical pass may consider checking memberships;
        // its winning files still require fresh helper verification before UI
        // publication. BLOB operations preserve paths with invalid UTF-8.
        let visible = if self.index_hidden && (self.hidden || self.query.scope != Scope::Indexed) {
            ""
        } else {
            " AND instr(substr(p.path,length(r.path)+(r.path<>x'2f')),x'2f2e')=0"
        };
        let availability = if self.show_offline {
            "(((m.verified=1 OR :pipeline='') AND r.available=1) OR (r.available=0 AND r.state='offline' AND NOT EXISTS(SELECT 1 FROM memberships am JOIN roots ar ON ar.id=am.root_id WHERE am.path=p.path AND ar.available=1)))"
        } else {
            "((m.verified=1 OR :pipeline='') AND r.available=1)"
        };
        let scope = match self.query.scope {
            Scope::Here => " AND p.parent=:directory",
            Scope::Recursive => " AND p.path>=:lower AND p.path<:upper",
            Scope::Indexed => "",
        };
        format!(
            "(:pipeline='' OR f.pipeline=:pipeline) AND f.extraction_version={version} AND f.state IN ('ready','unsupported') AND EXISTS(SELECT 1 FROM memberships m JOIN roots r ON r.id=m.root_id LEFT JOIN source_sessions ss ON ss.root_id=r.id WHERE m.path=p.path AND (ss.scope=f.scope OR (r.available=0 AND ss.root_id IS NULL)) AND {availability}{visible}){scope}",
            version = super::store::EXTRACTION_VERSION
        )
    }

    fn statement(
        &self,
        sql: &str,
        expression: Option<&str>,
    ) -> Result<rusqlite::CachedStatement<'_>, String> {
        let mut statement = self
            .store
            .connection
            .prepare_cached(sql)
            .map_err(|e| e.to_string())?;
        let directory = self.path.components().collect::<PathBuf>();
        let mut lower = directory.as_os_str().as_encoded_bytes().to_vec();
        if !lower.ends_with(b"/") {
            lower.push(b'/');
        }
        let mut upper = lower.clone();
        *upper.last_mut().unwrap() += 1;
        let directory = directory.as_os_str().as_encoded_bytes();
        let expression = expression.unwrap_or_default();
        let folded = self.text.to_lowercase();
        let values: &[(&str, &dyn rusqlite::ToSql)] = &[
            (":pipeline", &self.pipeline),
            (":directory", &directory),
            (":lower", &lower),
            (":upper", &upper),
            (":expression", &expression),
            (":folded", &folded),
        ];
        for (name, value) in values {
            if let Some(index) = statement.parameter_index(name).map_err(|e| e.to_string())? {
                statement
                    .raw_bind_parameter(index, *value)
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(statement)
    }
    fn path_matches(&self, current: &Path) -> bool {
        let scoped = match self.query.scope {
            Scope::Here => current.parent() == Some(self.path),
            Scope::Recursive => current.starts_with(self.path),
            Scope::Indexed => true,
        };
        scoped
            && !super::policy::excluded_by_user(current, self.exclusions)
            && super::policy::excluded(current).is_none()
            && !super::policy::secret_file(current)
    }
    fn candidate(&self, current: &Path, file_id: i64) -> Result<Option<Arc<Entry>>, String> {
        if !self.path_matches(current) {
            return Ok(None);
        }
        // Keep metadata out of the full-scope scan. Covering lookup indexes
        // admit paths/ranks first; only potentially retained entries need JSON.
        let mut statement = self
            .store
            .connection
            .prepare_cached("SELECT metadata FROM files WHERE id=?1")
            .map_err(|e| e.to_string())?;
        let json: String = statement
            .query_row([file_id], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        let mut entry: Entry = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        entry.path = current.into();
        entry.hidden = current
            .file_name()
            .is_some_and(|name| name.as_encoded_bytes().starts_with(b"."));
        entry.name = filesystem::display_path(Path::new(current.file_name().unwrap_or_default()));
        let visible = self.hidden
            || self.query.scope == Scope::Indexed
            || !super::policy::hidden_descendant(current, self.path);
        Ok((visible && self.query.metadata_matches(&entry)).then(|| Arc::new(entry)))
    }
    fn lexical_rank(
        &self,
        row: &rusqlite::Row<'_>,
        name: &str,
        folded: &str,
        keyword: bool,
    ) -> Result<Option<(u8, f64)>, String> {
        if keyword {
            return Ok(Some((3, -row.get::<_, f64>(2).map_err(|e| e.to_string())?)));
        }
        Ok(
            crate::file_manager::type_navigation::name_rank(name, folded)
                .map(|tier| (tier, if name == self.text { 1.0 } else { 0.0 })),
        )
    }
    fn lexical_names(
        &self,
        expression: Option<&str>,
    ) -> Result<Option<(CacheEpoch, LexicalNames)>, String> {
        let Some(expression) = expression else {
            return Ok(None);
        };
        // These bounded probes decide whether to invest in a hint cache. They
        // never restrict candidates or substitute for source/scope admission.
        let scope = match self.query.scope {
            Scope::Here => " WHERE p.parent=:directory",
            Scope::Recursive => " WHERE p.path>=:lower AND p.path<:upper",
            Scope::Indexed => "",
        };
        for (sql, expression, minimum) in [
            (
                format!("SELECT 1 FROM paths p{scope} LIMIT {LIMIT}"),
                None,
                LIMIT,
            ),
            (
                "SELECT 1 FROM segment_fts WHERE segment_fts MATCH :expression LIMIT 2048".into(),
                Some(expression),
                2048,
            ),
        ] {
            let mut statement = self.statement(&sql, expression)?;
            let mut rows = statement.raw_query();
            let mut count = 0;
            while rows.next().map_err(|error| error.to_string())?.is_some() {
                check(self.cancel)?;
                count += 1;
            }
            if count < minimum {
                return Ok(None);
            }
        }
        self.store.prepare_lexical_names(self.cancel).map(Some)
    }
    fn lexical(&self, expression: Option<&str>, result: &mut Top) -> Result<(), String> {
        let keyword = expression.is_some();
        let hints = self.lexical_names(expression)?;
        let select = if hints.is_some() {
            FTS_SELECT.replace("WHERE segment_fts MATCH", "WHERE clip_segment_might_rank(segment_fts.rowid,bm25(segment_fts)) AND segment_fts MATCH")
        } else if keyword {
            FTS_SELECT.into()
        } else {
            NAMES_SELECT.into()
        };
        let admission = LexicalRankFilter::with_paths(
            &self.store.connection,
            result,
            hints.as_ref().map(|(_, paths)| paths.clone()),
        )?;
        let mut statement = self.statement(&format!("{select}{}", self.current()), expression)?;
        let mut rows = statement.raw_query();
        let folded = self.text.to_lowercase();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            check(self.cancel)?;
            let path = row_path(row)?;
            let name =
                filesystem::display_path_cow(Path::new(path.file_name().unwrap_or_default()));
            let Some((tier, score)) = self.lexical_rank(row, &name, &folded, keyword)? else {
                continue;
            };
            let mut result = admission
                .top
                .lock()
                .map_err(|_| "Lexical ranking unavailable")?;
            if !result.accepts(path, &name, tier, score) {
                continue;
            }
            if let Some(entry) = self.candidate(path, row.get(1).map_err(|e| e.to_string())?)? {
                let segments = if keyword {
                    vec![SegmentMatch {
                        id: row.get(3).map_err(|e| e.to_string())?,
                        score,
                        keyword: true,
                    }]
                } else {
                    Vec::new()
                };
                result.offer(Ranked {
                    entry,
                    tier,
                    score,
                    segments,
                });
            }
        }
        drop(rows);
        drop(statement);
        drop(admission);
        if let Some((epoch, _)) = hints {
            self.store.finish_lexical_names(epoch)?;
        }
        Ok(())
    }
    fn semantic(&self) -> Result<BTreeMap<String, Top>, String> {
        let mut lists = BTreeMap::<String, Top>::new();
        let mut statement = self.statement(
            &format!("{}{}", self.semantic_select(), self.current()),
            None,
        )?;
        let mut rows = statement.raw_query();
        let mut checked = 0_u64;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            check(self.cancel)?;
            self.semantic_row(row, &mut lists)?;
            checked += 1;
            // One replaceable counter, not a queue of per-row notifications.
            // Counts admitted SQL references, including filtered/NULL rows and
            // aliases; do not present this as unique vectors or a percentage.
            if checked.is_multiple_of(2048)
                && let Some(progress) = self.progress
            {
                progress.store(checked, AtomicOrdering::Relaxed);
            }
        }
        if let Some(progress) = self.progress {
            progress.store(checked, AtomicOrdering::Relaxed);
        }
        Ok(lists)
    }
    fn semantic_select(&self) -> &'static str {
        match self.query.scope {
            Scope::Indexed => INDEXED_SEMANTIC_SELECT,
            _ => SEMANTIC_SELECT,
        }
    }
    fn semantic_floor(&self, modality: &str) -> Option<f32> {
        match modality {
            "text" | "image" | "audio" => {
                Some(self.floors.get(modality).copied().unwrap_or_else(|| {
                    little_clip_core::config::FileSearchConfig::default_semantic_floor(modality)
                }))
            }
            _ => None,
        }
    }
    fn score_candidate(
        &self,
        row: &rusqlite::Row<'_>,
        path: &Path,
        floor: f32,
    ) -> Result<Option<SemanticScore>, String> {
        let encoded = row.get_ref(3).map_err(|error| error.to_string())?;
        // Cold NULLs are lexical-only segments; a distinct integer marks an
        // already validated cache hit. Keep NULL rows out of metadata/scoring.
        if matches!(encoded, rusqlite::types::ValueRef::Null) {
            return Ok(None);
        }
        // Restrictive metadata still filters before scoring, including corrupt
        // vectors outside those constraints. Unconstrained queries decode only
        // the metadata of candidates that can contribute to an admitted list.
        let entry = if self.query.has_selectors() {
            let Some(entry) = self.candidate(path, row.get(1).map_err(|e| e.to_string())?)? else {
                return Ok(None);
            };
            Some(entry)
        } else {
            None
        };
        let encoded = match encoded {
            rusqlite::types::ValueRef::Integer(1) => None,
            rusqlite::types::ValueRef::Blob(bytes) => Some(bytes),
            _ => return Err("Invalid vector storage".into()),
        };
        let score = self.store.score_vector(
            row.get(4).map_err(|error| error.to_string())?,
            self.vector,
            encoded,
        )?;
        Ok((score >= floor).then_some(SemanticScore {
            value: score,
            entry,
        }))
    }
    fn semantic_row(
        &self,
        row: &rusqlite::Row<'_>,
        lists: &mut BTreeMap<String, Top>,
    ) -> Result<(), String> {
        let path = row_path(row)?;
        if !self.path_matches(path) {
            return Ok(());
        }
        let modality = row_text(row, 2)?;
        let Some(floor) = self.semantic_floor(modality) else {
            return Ok(());
        };
        let Some(semantics) = self.score_candidate(row, path, floor)? else {
            return Ok(());
        };
        let score = semantics.value;
        let name = filesystem::display_path_cow(Path::new(path.file_name().unwrap_or_default()));
        let list = lists.entry(modality.into()).or_default();
        if !list.accepts(path, &name, 0, f64::from(score)) {
            return Ok(());
        }
        let entry = match semantics.entry {
            Some(entry) => entry,
            None => {
                let Some(entry) = self.candidate(path, row.get(1).map_err(|e| e.to_string())?)?
                else {
                    return Ok(());
                };
                entry
            }
        };
        list.offer(Ranked {
            entry,
            tier: 0,
            score: f64::from(score),
            segments: vec![SegmentMatch {
                id: row.get(4).map_err(|e| e.to_string())?,
                score: f64::from(score),
                keyword: false,
            }],
        });
        Ok(())
    }
}
fn row_path<'a>(row: &'a rusqlite::Row<'_>) -> Result<&'a Path, String> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = row
        .get_ref(0)
        .map_err(|e| e.to_string())?
        .as_blob()
        .map_err(|e| e.to_string())?;
    Ok(Path::new(std::ffi::OsStr::from_bytes(bytes)))
}
fn row_text<'a>(row: &'a rusqlite::Row<'_>, column: usize) -> Result<&'a str, String> {
    row.get_ref(column)
        .map_err(|e| e.to_string())?
        .as_str()
        .map_err(|e| e.to_string())
}

fn fuse(lexical: &Top, semantics: &BTreeMap<String, Top>, text: &str, partial: bool) -> Vec<Hit> {
    let mut fused = HashMap::<PathBuf, Ranked>::new();
    let folded = text.to_lowercase();
    // Partial-keyword rescue is weaker evidence than a match in any semantic
    // modality. Scale it with the per-modality weight so a mixed-media folder
    // cannot promote incidental words merely by having more modalities.
    let lexical_weight = if partial {
        0.5 / semantics.len().max(1) as f64
    } else {
        2.0
    };
    for (weight, list) in std::iter::once((lexical_weight, lexical)).chain(
        semantics
            .values()
            .map(|list| (1.0 / semantics.len() as f64, list)),
    ) {
        for (rank, item) in list.best().enumerate() {
            let result = fused
                .entry(item.entry.path.clone())
                .or_insert_with(|| Ranked {
                    entry: item.entry.clone(),
                    tier: if item.entry.name == text {
                        0
                    } else if item.entry.name.to_lowercase() == folded {
                        1
                    } else {
                        2
                    },
                    score: 0.0,
                    segments: Vec::new(),
                });
            result.score += weight / (61.0 + rank as f64);
            explanation::merge(&mut result.segments, &item.segments);
        }
    }
    let mut ranked = fused.into_values().collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.cmp(a));
    ranked.truncate(LIMIT);
    ranked
        .into_iter()
        .map(|item| Hit {
            entry: (*item.entry).clone(),
            segments: item.segments,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_manager::{
        filesystem::read_directory,
        search::{
            policy::Root,
            store::{DIMENSION, Segment},
        },
    };
    use rusqlite::OptionalExtension;
    struct Fixture {
        _cache: tempfile::TempDir,
        source: tempfile::TempDir,
        store: Store,
        root: i64,
        generation: i64,
        exclusions: Vec<PathBuf>,
        index_hidden: bool,
        show_offline: bool,
    }
    impl Fixture {
        fn new() -> Self {
            let cache = tempfile::tempdir().unwrap();
            let source = tempfile::tempdir().unwrap();
            let mut store = Store::open(cache.path()).unwrap();
            let root = store.enroll(&Root::open(source.path()).unwrap()).unwrap();
            let generation = store.begin_scan(root).unwrap();
            Self {
                _cache: cache,
                source,
                store,
                root,
                generation,
                exclusions: Vec::new(),
                index_hidden: true,
                show_offline: false,
            }
        }
        fn add(
            &mut self,
            name: impl AsRef<Path>,
            text: &str,
            score: Option<f32>,
            duplicates: usize,
        ) -> Arc<Entry> {
            let path = self.source.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            let entry = read_directory(path.parent().unwrap(), &AtomicBool::new(false))
                .unwrap()
                .entries
                .into_iter()
                .find(|entry| entry.path == path)
                .unwrap();
            let (id, _) = self
                .store
                .observe(
                    self.root,
                    self.generation,
                    &entry,
                    &std::fs::metadata(&path).unwrap(),
                    "model",
                )
                .unwrap();
            let segments = score
                .map(|score| {
                    let mut vector = vec![0.0; DIMENSION];
                    vector[0] = score;
                    vector[1] = (1.0 - score * score).sqrt();
                    (0..duplicates)
                        .map(|i| Segment {
                            modality: "text".into(),
                            location: format!("passage {i}"),
                            text: text.into(),
                            input_hash: vec![1; 32],
                            vector: Some(vector.clone()),
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            self.store
                .publish(id, entry.revision, "model", &segments, "complete")
                .unwrap();
            Arc::new(entry)
        }
        fn find(
            &self,
            description: &str,
            scope: &Path,
            selectors: &str,
            hidden: bool,
        ) -> Vec<String> {
            let (head, options) = selectors.split_once(';').unwrap_or((selectors, ""));
            let command = format!(":{head};emb:\"{description}\";{options}");
            let query = Query::parse(command.trim_end_matches(';')).unwrap();
            let mut vector = vec![0.0; DIMENSION];
            vector[0] = 1.0;
            Retrieval {
                progress: None,
                store: &self.store,
                pipeline: "model",
                vector: &vector,
                path: scope,
                query: &query,
                text: description,
                hidden,
                index_hidden: self.index_hidden,
                show_offline: self.show_offline,
                exclusions: &self.exclusions,
                // Synthetic scores test ranking/admission independently of
                // empirical model cutoffs. Default behavior is covered below.
                floors: &BTreeMap::from([("text".into(), 0.25)]),
                cancel: &Arc::new(AtomicBool::new(false)),
            }
            .search()
            .unwrap()
            .into_iter()
            .map(|hit| hit.entry.name)
            .collect()
        }
    }
    #[test]
    fn coarse_names_count_distinct_paths_and_retire_the_last_owner() {
        let mut fixture = Fixture::new();
        let seed = fixture.add("same.txt", "ordinary words", Some(1.0), 1);
        let mut top = Top {
            by_name: Some(HashMap::new()),
            ..Top::default()
        };
        for index in 0..LIMIT {
            let mut entry = (*seed).clone();
            entry.path = fixture.source.path().join(format!("dir_{index}/same.txt"));
            top.offer(Ranked {
                entry: Arc::new(entry),
                tier: 3,
                score: 1.0,
                segments: Vec::new(),
            });
        }
        assert_eq!(top.by_name.as_ref().unwrap()["same.txt"], LIMIT);
        assert!(top.accepts_name("same.txt", -1.0));
        assert!(!top.accepts_name("z.txt", -1.0));
        let paths = top.by_path.keys().cloned().collect::<Vec<_>>();
        for (index, path) in paths.iter().enumerate() {
            top.remove(path).unwrap();
            assert_eq!(
                top.by_name
                    .as_ref()
                    .unwrap()
                    .get("same.txt")
                    .copied()
                    .unwrap_or(0),
                LIMIT - index - 1
            );
        }
        assert!(top.by_name.as_ref().unwrap().is_empty());
        let mut ordinary = Top::default();
        ordinary.offer(Ranked {
            entry: seed,
            tier: 3,
            score: 1.0,
            segments: Vec::new(),
        });
        assert!(
            ordinary.by_name.is_none(),
            "semantic scoring must not allocate this lexical-only accelerator"
        );
    }

    #[test]
    fn broad_lexical_hints_match_alias_fallback_order_and_every_retained_location() {
        let mut fixture = Fixture::new();
        for index in 0..520 {
            fixture.add(
                format!("record_{index}.txt"),
                "shared ordinary words",
                Some(0.9),
                4,
            );
        }
        let query = Query::parse(":file").unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let floors = BTreeMap::new();
        let retrieval = Retrieval {
            store: &fixture.store,
            pipeline: "model",
            vector: &[],
            path: fixture.source.path(),
            query: &query,
            text: "shared",
            hidden: false,
            index_hidden: true,
            show_offline: false,
            exclusions: &[],
            floors: &floors,
            cancel: &cancel,
            progress: None,
        };
        let signature = |hits: Vec<Hit>| {
            hits.into_iter()
                .map(|hit| {
                    (
                        hit.entry.path,
                        hit.segments
                            .into_iter()
                            .map(|segment| (segment.id, segment.score.to_bits(), segment.keyword))
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>()
        };
        let expected = signature(retrieval.search().unwrap());
        assert_eq!(expected.len(), LIMIT);
        assert!(
            expected
                .iter()
                .all(|(_, segments)| segments.len() == explanation::MAX_PASSAGES)
        );
        assert_eq!(
            fixture
                .store
                .prepare_lexical_names(&cancel)
                .unwrap()
                .1
                .len(),
            2080
        );
        // All aliases are outside Here scope, but must disable singleton hints
        // for their file. This exercises the real SQL fallback without a test
        // switch or changing the candidate set/ranking.
        use std::os::unix::ffi::OsStrExt;
        let alias_parent = fixture.source.path().join("nested");
        let aliases: Vec<(i64, Vec<u8>)> = {
            let mut statement = fixture
                .store
                .connection
                .prepare("SELECT file_id,path FROM paths")
                .unwrap();
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        for (id, bytes) in aliases {
            let name = Path::new(std::ffi::OsStr::from_bytes(&bytes))
                .file_name()
                .unwrap();
            let alias = alias_parent.join(name);
            fixture
                .store
                .connection
                .execute(
                    "INSERT INTO paths(path,parent,file_id) VALUES(?1,?2,?3)",
                    rusqlite::params![
                        alias.as_os_str().as_encoded_bytes(),
                        alias_parent.as_os_str().as_encoded_bytes(),
                        id
                    ],
                )
                .unwrap();
        }
        assert_eq!(signature(retrieval.search().unwrap()), expected);
        assert!(
            fixture
                .store
                .prepare_lexical_names(&cancel)
                .unwrap()
                .1
                .is_empty()
        );
    }

    #[test]
    fn warm_vectors_follow_local_external_and_policy_changes_and_idle_release() {
        let mut fixture = Fixture::new();
        fixture.add("a.txt", "ordinary text", Some(0.9), 1);
        fixture.add("b.txt", "ordinary text", Some(0.8), 1);
        let path = fixture.source.path().to_owned();
        assert_eq!(
            fixture.find("abstract meaning", &path, "file", false),
            ["a.txt", "b.txt"]
        );
        assert_eq!(fixture.store.vector_cache_usage().0, 2);
        fixture.exclusions.push(path.join("a.txt"));
        assert_eq!(
            fixture.find("abstract meaning", &path, "file", false),
            ["b.txt"]
        );
        fixture.exclusions.clear();
        let mut low = vec![0.0; DIMENSION];
        low[1] = 1.0;
        let low = super::super::store::encode_vector(&low).unwrap();
        fixture
            .store
            .connection
            .execute(
                "UPDATE segments SET vector=?1 WHERE id=(SELECT min(id) FROM segments)",
                [&low],
            )
            .unwrap();
        assert_eq!(
            fixture.find("abstract meaning", &path, "file", false),
            ["b.txt"]
        );
        let external =
            rusqlite::Connection::open(fixture._cache.path().join("index.sqlite")).unwrap();
        external
            .execute("UPDATE segments SET vector=?1", [&low])
            .unwrap();
        assert!(
            fixture
                .find("abstract meaning", &path, "file", false)
                .is_empty()
        );
        assert_eq!(fixture.store.vector_cache_usage().0, 2);
        let epoch = fixture.store.prepare_vector_cache().unwrap();
        external
            .execute("UPDATE roots SET available=0", [])
            .unwrap();
        assert!(fixture.store.finish_vector_cache(epoch).is_err());
        assert_eq!(fixture.store.vector_cache_usage().0, 0);
        assert!(
            fixture
                .find("abstract meaning", &path, "file", false)
                .is_empty()
        );
        fixture
            .store
            .release_idle_vectors(std::time::Duration::ZERO);
        assert_eq!(fixture.store.vector_cache_usage(), (0, 0));
    }

    #[test]
    fn warmed_cache_keeps_hard_filters_before_corrupt_vector_decoding() {
        let mut fixture = Fixture::new();
        fixture.add("a.txt", "ordinary text", Some(0.9), 1);
        let path = fixture.source.path().to_owned();
        assert_eq!(
            fixture.find("abstract meaning", &path, "file", false),
            ["a.txt"]
        );
        let nan = half::f16::NAN.to_le_bytes().repeat(DIMENSION);
        fixture
            .store
            .connection
            .execute("UPDATE segments SET vector=?1", [&nan])
            .unwrap();
        let filtered = Query::parse(r#":image;emb:"abstract meaning""#).unwrap();
        let unfiltered = Query::parse(r#":emb:"abstract meaning""#).unwrap();
        let mut vector = vec![0.0; DIMENSION];
        vector[0] = 1.0;
        let mut retrieval = Retrieval {
            progress: None,
            store: &fixture.store,
            pipeline: "model",
            vector: &vector,
            path: &path,
            query: &filtered,
            text: "abstract meaning",
            hidden: false,
            index_hidden: false,
            show_offline: false,
            exclusions: &[],
            floors: &BTreeMap::new(),
            cancel: &Arc::new(AtomicBool::new(false)),
        };
        assert!(retrieval.search().unwrap().is_empty());
        assert_eq!(fixture.store.vector_cache_usage().0, 0);
        retrieval.query = &unfiltered;
        assert!(retrieval.search().is_err());
        // NULL vector rows remain lexical-only, even if unrelated metadata is
        // malformed. The covering scan must not add semantic metadata reads.
        fixture
            .store
            .connection
            .execute_batch(
                "UPDATE segments SET vector=NULL; UPDATE files SET metadata='invalid JSON';",
            )
            .unwrap();
        retrieval.query = &filtered;
        assert!(retrieval.search().unwrap().is_empty());
    }

    #[test]
    fn lexical_sql_threshold_preserves_ties_retained_paths_and_error_cleanup() {
        let mut fixture = Fixture::new();
        let seed = fixture.add("seed.txt", "ordinary words", Some(1.0), 1);
        let mut top = Top::default();
        for index in 1000..1500 {
            let mut entry = (*seed).clone();
            entry.name = format!("foo_{index}.txt");
            entry.path = fixture.source.path().join(&entry.name);
            top.offer(Ranked {
                entry: Arc::new(entry),
                tier: 3,
                score: 1.0,
                segments: Vec::new(),
            });
        }
        let connection = &fixture.store.connection;
        let accepts = |name: &str, score: f64| {
            let path = fixture.source.path().join(name);
            connection
                .query_row(
                    "SELECT clip_lexical_might_rank(?1,?2)",
                    rusqlite::params![path.as_os_str().as_encoded_bytes(), -score],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap()
        };
        let result = (|| -> Result<(), String> {
            let paths = Arc::new(HashMap::from([
                (1, Box::<str>::from("foo_2.txt")),
                (2, Box::<str>::from("foo_2000.txt")),
                (3, Box::<str>::from("foo_1499.txt")),
            ]));
            let filter = LexicalRankFilter::with_paths(connection, &mut top, Some(paths))?;
            let segment_accepts = |id: i64, score: f64| {
                connection
                    .query_row(
                        "SELECT clip_segment_might_rank(?1,?2)",
                        rusqlite::params![id, -score],
                        |row| row.get::<_, bool>(0),
                    )
                    .unwrap()
            };
            assert!(segment_accepts(1, 1.0));
            assert!(!segment_accepts(2, 1.0));
            assert!(segment_accepts(3, 0.1));
            assert!(
                segment_accepts(999, 0.0),
                "uncached and aliased segments must fall through to ordinary SQL admission"
            );
            assert!(accepts("foo_2.txt", 1.0));
            assert!(!accepts("foo_2000.txt", 1.0));
            assert!(
                accepts("foo_1499.txt", 0.1),
                "retained paths still merge lower-scoring locations"
            );
            let mut earlier = (*seed).clone();
            earlier.name = "foo_2.txt".into();
            earlier.path = fixture.source.path().join(&earlier.name);
            filter.top.lock().unwrap().offer(Ranked {
                entry: Arc::new(earlier),
                tier: 3,
                score: 1.0,
                segments: Vec::new(),
            });
            assert!(
                !accepts("foo_1499.txt", 0.1),
                "evicted paths must obey the current boundary"
            );
            assert!(!segment_accepts(3, 0.1));
            connection
                .query_row(
                    "SELECT clip_lexical_might_rank('invalid TEXT path',0)",
                    [],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(|error| error.to_string())?;
            Ok(())
        })();
        assert!(result.is_err());
        assert!(
            connection
                .query_row("SELECT clip_segment_might_rank(1,0)", [], |row| row
                    .get::<_, bool>(0))
                .is_err()
        );
        assert_eq!(top.by_path.len(), LIMIT);
        assert!(
            top.by_path
                .contains_key(&fixture.source.path().join("foo_2.txt"))
        );
        assert!(
            connection
                .query_row("SELECT clip_lexical_might_rank(x'2f',0)", [], |row| row
                    .get::<_, bool>(0))
                .is_err(),
            "the query-local callback must be removed on error"
        );
    }

    #[test]
    fn default_cutoffs_reject_weak_scores_and_explicit_overrides_remain_available() {
        let mut fixture = Fixture::new();
        fixture.add("candidate.txt", "unrelated source words", Some(0.5), 1);
        let query = Query::parse(r#":emb:"abstract meaning""#).unwrap();
        let mut vector = vec![0.0; DIMENSION];
        vector[0] = 1.0;
        let defaults = BTreeMap::new();
        let mut retrieval = Retrieval {
            progress: None,
            store: &fixture.store,
            pipeline: "model",
            vector: &vector,
            path: fixture.source.path(),
            query: &query,
            text: "abstract meaning",
            hidden: false,
            index_hidden: false,
            show_offline: false,
            exclusions: &[],
            floors: &defaults,
            cancel: &Arc::new(AtomicBool::new(false)),
        };
        assert!(retrieval.search().unwrap().is_empty());
        let override_floors = BTreeMap::from([("text".into(), 0.4)]);
        retrieval.floors = &override_floors;
        assert_eq!(retrieval.search().unwrap()[0].entry.name, "candidate.txt");
    }

    #[test]
    fn incidental_words_in_long_descriptions_do_not_crowd_out_semantic_matches() {
        let mut fixture = Fixture::new();
        for index in 0..24 {
            fixture.add(
                format!("noise-{index}.txt"),
                "A document with unrelated inventory notes.",
                Some(0.1),
                1,
            );
        }
        fixture.add("visual.png", "", Some(0.95), 1);
        assert_eq!(
            fixture.find(
                "a picture with three large blue circles",
                fixture.source.path(),
                "",
                false
            ),
            vec!["visual.png"]
        );
    }

    #[test]
    fn partial_words_in_short_queries_do_not_crowd_out_semantic_matches() {
        let mut fixture = Fixture::new();
        for index in 0..24 {
            fixture.add(
                format!("noise-{index}.txt"),
                "A document with unrelated inventory notes.",
                Some(0.1),
                1,
            );
        }
        fixture.add("recording.wav", "", Some(0.95), 1);
        let names = fixture.find("A dog barking", fixture.source.path(), "", false);
        assert_eq!(names[0], "recording.wav");
        assert_eq!(names.len(), 25, "Partial keyword results remain available");
        // Single-letter and non-English tokens keep literal FTS behavior.
        fixture.add("language.txt", "C भाषा", Some(0.1), 1);
        assert_eq!(
            fixture.find("C भाषा", fixture.source.path(), "", false)[0],
            "language.txt"
        );
    }

    #[test]
    fn short_keyword_queries_keep_the_broad_keyword_fallback() {
        let mut fixture = Fixture::new();
        fixture.add(
            "payment.txt",
            "Invoice for replacing panes of glass.",
            Some(0.1),
            1,
        );
        assert_eq!(
            fixture.find("window invoice", fixture.source.path(), "", false),
            vec!["payment.txt"]
        );
    }

    #[test]
    fn sqlite_name_prefilter_keeps_unicode_and_escaped_unix_filenames() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        register_name_filter(&connection).unwrap();
        let cases: &[(&[u8], &str, bool)] = &[
            (b"/root/Report.txt", "report", true),
            ("/root/Résumé.pdf".as_bytes(), "résumé", true),
            (b"/root/take_\xFF.wav", r"\xff", true),
            (b"/root/take\\part.wav", r"take\\part", true),
            (b"/root/take\npart.wav", r"\x0a", true),
            (b"/root/report.txt", "unrelated", false),
        ];
        for (path, query, expected) in cases {
            let actual: bool = connection
                .query_row(
                    "SELECT clip_filename_matches(?1,?2)",
                    rusqlite::params![path, query],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(actual, *expected, "path={path:?}, query={query:?}");
        }
    }

    #[test]
    fn cancellation_interrupts_sql_without_a_result_row_and_releases_the_connection() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let started = std::time::Instant::now();
        let result = std::thread::scope(|scope| {
            let _guard = SqlCancellation::new(&connection, &cancel);
            scope.spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(20));
                cancel.store(true, AtomicOrdering::Relaxed);
            });
            connection.query_row(
                "WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<100000000) SELECT sum(x) FROM n",
                [], |row| row.get::<_, i64>(0),
            )
        });
        assert!(
            matches!(result, Err(rusqlite::Error::SqliteFailure(error, _))
            if error.code == rusqlite::ErrorCode::OperationInterrupted)
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
        // Cancellation belongs to the finished read, never to later accepted writes.
        assert!(cancel.load(AtomicOrdering::Relaxed));
        connection
            .execute_batch("CREATE TABLE later(value); INSERT INTO later VALUES(1)")
            .unwrap();
        assert_eq!(
            connection
                .query_row("SELECT value FROM later", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn invalid_query_vectors_cannot_silently_change_semantic_ranking() {
        let fixture = Fixture::new();
        let query = Query::parse(r#":emb:"meaning""#).unwrap();
        let invalid = [
            vec![1.0],
            vec![0.0; DIMENSION],
            vec![f32::NAN; DIMENSION],
            vec![f32::INFINITY; DIMENSION],
        ];
        for vector in invalid {
            let result = Retrieval {
                progress: None,
                store: &fixture.store,
                pipeline: "model",
                vector: &vector,
                path: fixture.source.path(),
                query: &query,
                text: "meaning",
                hidden: false,
                index_hidden: false,
                show_offline: false,
                exclusions: &[],
                floors: &BTreeMap::new(),
                cancel: &Arc::new(AtomicBool::new(false)),
            }
            .search();
            assert!(
                result
                    .err()
                    .unwrap()
                    .contains("finite nonzero 768-dimensional")
            );
        }
    }

    #[test]
    fn cached_credentials_cannot_supply_lexical_or_semantic_results() {
        let mut fixture = Fixture::new();
        fixture.add("credentials.json", "violet lighthouse", Some(1.0), 1);
        fixture.add("secret.key", "violet lighthouse", Some(1.0), 1);
        fixture.add(".env", "violet lighthouse", Some(1.0), 1);
        fixture.add("ordinary.txt", "violet lighthouse", Some(0.8), 1);
        assert_eq!(
            fixture.find("violet", fixture.source.path(), "", true),
            vec!["ordinary.txt"]
        );
    }

    #[test]
    fn sql_scope_keeps_component_boundaries_and_binary_path_bytes() {
        use std::os::unix::ffi::OsStringExt;
        let mut fixture = Fixture::new();
        let directory = PathBuf::from(std::ffi::OsString::from_vec(b"group\xff".to_vec()));
        fixture.add(directory.join("a.txt"), "needle content", Some(0.9), 1);
        fixture.add(directory.join("deep/b.txt"), "needle content", Some(0.8), 1);
        fixture.add("group-neighbor/c.txt", "needle content", Some(0.7), 1);
        let scope = fixture.source.path().join(&directory);
        assert_eq!(fixture.find("needle", &scope, "", false), vec!["a.txt"]);
        assert_eq!(
            fixture.find("needle", &scope.join(""), ";scope:recursive", false),
            vec!["a.txt", "b.txt"]
        );
        assert_eq!(
            fixture.find("needle", Path::new("/"), ";scope:recursive", true),
            vec!["a.txt", "b.txt", "c.txt"]
        );
    }

    #[test]
    fn live_and_offline_rows_cannot_cross_a_known_source_context() {
        let mut fixture = Fixture::new();
        fixture.add("note.txt", "violet lighthouse", Some(0.9), 1);
        fixture
            .store
            .connection
            .execute("UPDATE files SET scope='different-source'", [])
            .unwrap();
        assert!(
            fixture
                .find("violet", fixture.source.path(), "", false)
                .is_empty()
        );
        fixture
            .store
            .unavailable(fixture.source.path(), "disconnected")
            .unwrap();
        fixture.show_offline = true;
        assert!(
            fixture
                .find("violet", fixture.source.path(), "", false)
                .is_empty()
        );
    }

    #[test]
    fn offline_results_require_opt_in_and_do_not_revive_excluded_or_disabled_roots() {
        let mut fixture = Fixture::new();
        let entry = fixture.add("old.txt", "violet lighthouse", Some(0.8), 1);
        fixture.add(".git/secret.txt", "violet lighthouse", Some(1.0), 1);
        fixture
            .store
            .unavailable(fixture.source.path(), "disconnected")
            .unwrap();
        assert!(
            fixture
                .find("violet", fixture.source.path(), ";scope:indexed", false)
                .is_empty()
        );
        fixture.show_offline = true;
        assert_eq!(
            fixture.find("violet", fixture.source.path(), ";scope:indexed", false),
            ["old.txt"]
        );
        assert!(
            fixture
                .store
                .last_known_paths(&[(*entry).clone()])
                .unwrap()
                .contains(&entry.path)
        );
        fixture.exclusions.push(entry.path.clone());
        assert!(
            fixture
                .find("violet", fixture.source.path(), ";scope:indexed", false)
                .is_empty()
        );
        fixture.exclusions.clear();
        fixture.store.disable_unconfigured(&[]).unwrap();
        assert!(
            fixture
                .find("violet", fixture.source.path(), ";scope:indexed", false)
                .is_empty()
        );
        assert!(
            fixture
                .store
                .last_known_paths(&[(*entry).clone()])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn an_available_overlapping_membership_cannot_fall_back_to_stale_offline_evidence() {
        let mut fixture = Fixture::new();
        let entry = fixture.add("child/old.txt", "violet lighthouse", Some(0.8), 1);
        let child = fixture
            .store
            .enroll(&Root::open(&fixture.source.path().join("child")).unwrap())
            .unwrap();
        let generation = fixture.store.begin_scan(child).unwrap();
        fixture
            .store
            .observe(
                child,
                generation,
                &entry,
                &std::fs::metadata(&entry.path).unwrap(),
                "model",
            )
            .unwrap();
        fixture
            .store
            .unavailable(fixture.source.path(), "disconnected parent source")
            .unwrap();
        assert!(
            fixture
                .store
                .last_known_paths(&[(*entry).clone()])
                .unwrap()
                .is_empty()
        );
        fixture
            .store
            .unavailable(
                &fixture.source.path().join("child"),
                "disconnected child source",
            )
            .unwrap();
        assert_eq!(
            fixture
                .store
                .last_known_paths(&[(*entry).clone()])
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn semantic_progress_covers_all_admitted_references_and_partial_tail() {
        let mut fixture = Fixture::new();
        let a = fixture.add("a.txt", "ordinary content", Some(0.9), 1500);
        fixture.add("b.txt", "ordinary content", Some(0.9), 1500);
        let c = fixture.add("c.txt", "ordinary content", Some(0.9), 1500);
        for (source, name) in [
            (&a.path, "a-copy.txt"),
            (&c.path, "c-stale.txt"),
            (&c.path, ".c-hidden.txt"),
        ] {
            std::fs::hard_link(source, fixture.source.path().join(name)).unwrap();
            fixture.add(name, "ordinary content", Some(0.9), 1500);
        }
        let stale = fixture.source.path().join("c-stale.txt");
        fixture
            .store
            .connection
            .execute(
                "UPDATE memberships SET verified=0 WHERE path=?1",
                [stale.as_os_str().as_encoded_bytes()],
            )
            .unwrap();
        fixture.add("name-only.zip", "opaque inventory", None, 0);
        let query = Query::parse(":emb:\"abstract meaning\"").unwrap();
        let vector = std::iter::once(1.0)
            .chain(std::iter::repeat_n(0.0, DIMENSION - 1))
            .collect::<Vec<_>>();
        let progress = AtomicU64::new(0);
        let cancel = Arc::new(AtomicBool::new(false));
        let floors = BTreeMap::new();
        let mut retrieval = Retrieval {
            store: &fixture.store,
            pipeline: "model",
            vector: &vector,
            path: fixture.source.path(),
            query: &query,
            text: "abstract meaning",
            hidden: false,
            index_hidden: false,
            show_offline: false,
            exclusions: &[],
            floors: &floors,
            cancel: &cancel,
            progress: Some(&progress),
        };
        assert_eq!(retrieval.search().unwrap().len(), 4);
        // One verified alias adds references, not unique vectors. A stale or
        // hidden alias must not contribute references or leak shared metadata.
        assert_eq!(progress.load(AtomicOrdering::Relaxed), 6000);
        let indexed = Query::parse(":emb:\"abstract meaning\";scope:indexed").unwrap();
        retrieval.query = &indexed;
        assert_eq!(retrieval.search().unwrap().len(), 4);
        assert_eq!(progress.load(AtomicOrdering::Relaxed), 6000);
        retrieval.query = &query;
        // SQL scope filtering precedes the counter; neither the result window
        // nor a prior cache hit ends scanning early.
        let here = fixture.source.path().join("empty");
        retrieval.path = &here;
        assert!(retrieval.search().unwrap().is_empty());
        assert_eq!(progress.load(AtomicOrdering::Relaxed), 0);
        retrieval.path = fixture.source.path();
        cancel.store(true, AtomicOrdering::Relaxed);
        assert!(retrieval.search().is_err());
        assert_eq!(progress.load(AtomicOrdering::Relaxed), 0);
    }

    #[test]
    fn lexical_search_needs_no_model_but_rejects_obsolete_extraction_records() {
        let mut fixture = Fixture::new();
        fixture.add("current.txt", "violet lighthouse", Some(0.0), 1);
        fixture.add("old.txt", "violet lighthouse", Some(1.0), 1);
        fixture.store.connection.execute("UPDATE files SET extraction_version=0 WHERE id=(SELECT file_id FROM paths WHERE path=?1)", [fixture.source.path().join("old.txt").as_os_str().as_encoded_bytes()]).unwrap();
        let query = Query::parse(":emb:\"violet lighthouse\"").unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let floors = BTreeMap::new();
        let mut retrieval = Retrieval {
            progress: None,
            store: &fixture.store,
            pipeline: "",
            vector: &[],
            path: fixture.source.path(),
            query: &query,
            text: "violet lighthouse",
            hidden: false,
            index_hidden: true,
            show_offline: false,
            exclusions: &[],
            floors: &floors,
            cancel: &cancel,
        };
        let hits = retrieval.search().unwrap();
        assert_eq!(
            hits.iter()
                .map(|hit| hit.entry.name.as_str())
                .collect::<Vec<_>>(),
            ["current.txt"]
        );
        assert!(hits[0].segments.iter().all(|segment| segment.keyword));
        fixture
            .store
            .connection
            .execute("UPDATE memberships SET verified=0", [])
            .unwrap();
        assert_eq!(
            retrieval.search().unwrap().len(),
            1,
            "checking inventory can supply lexical candidates for fresh source verification"
        );
        let mut vector = vec![0.0; DIMENSION];
        vector[0] = 1.0;
        retrieval.vector = &vector;
        assert!(retrieval.search().is_err());
        retrieval.pipeline = "model";
        assert!(
            retrieval.search().unwrap().is_empty(),
            "unverified inventory must not contribute semantic scoring"
        );
    }

    #[test]
    fn batched_keyword_evidence_preserves_fts_prefix_and_unicode_snippets() {
        let mut fixture = Fixture::new();
        let entry = fixture.add(
            "evidence.txt",
            &format!(
                "{}Violet lighthouse café guides boats.{}",
                "context ".repeat(100),
                " tail".repeat(100)
            ),
            Some(0.9),
            1,
        );
        let id: i64 = fixture
            .store
            .connection
            .query_row("SELECT id FROM segments LIMIT 1", [], |row| row.get(0))
            .unwrap();
        let hits = vec![Hit {
            entry: (*entry).clone(),
            segments: vec![SegmentMatch {
                id,
                score: 0.9,
                keyword: true,
            }],
        }];
        let cancel = Arc::new(AtomicBool::new(false));
        for text in [
            "violet",
            "CAFÉ",
            "cafe",
            "lighthou",
            "absent",
            "violet lighthouse",
        ] {
            let expression = terms(text).unwrap().join(" OR ");
            let expected: Option<String> = fixture.store.connection.query_row(
                "SELECT snippet(segment_fts,0,'','',' … ',32) FROM segment_fts WHERE rowid=?1 AND segment_fts MATCH ?2",
                rusqlite::params![id, expression], |row| row.get(0),
            ).optional().unwrap();
            let details = explanation::read(
                &fixture.store,
                &hits,
                &[(*entry).clone()],
                "model",
                text,
                &cancel,
            )
            .unwrap();
            let expected = expected.map(|value| {
                explanation::clipped(&value.split_whitespace().collect::<Vec<_>>().join(" "), 320)
            });
            assert_eq!(
                details[&entry.path]
                    .passages
                    .first()
                    .map(|passage| &passage.text),
                expected.as_ref(),
                "{text}"
            );
        }
    }

    #[test]
    fn explanations_read_only_verified_files_and_their_own_matching_segments() {
        let mut fixture = Fixture::new();
        let text = format!(
            "{}Violet lighthouse guides boats.{}",
            "ordinary context ".repeat(100),
            " trailing".repeat(100)
        );
        let first = fixture.add("one.txt", &text, Some(0.9), 5);
        let other = fixture.add("two.txt", "Unrelated private passage", Some(0.8), 1);
        let ids = fixture.store.connection.prepare("SELECT s.id FROM segments s JOIN paths p ON p.file_id=s.file_id WHERE p.path=?1 ORDER BY s.id")
            .unwrap().query_map([first.path.as_os_str().as_encoded_bytes()], |row| row.get::<_, i64>(0)).unwrap()
            .collect::<Result<Vec<_>, _>>().unwrap();
        let mut hits = vec![
            Hit {
                entry: (*first).clone(),
                segments: ids
                    .iter()
                    .map(|id| SegmentMatch {
                        id: *id,
                        score: 0.9,
                        keyword: true,
                    })
                    .collect(),
            },
            Hit {
                entry: (*other).clone(),
                segments: vec![SegmentMatch {
                    id: ids[0],
                    score: 0.8,
                    keyword: false,
                }],
            },
        ];
        let cancel = Arc::new(AtomicBool::new(false));
        let details = explanation::read(
            &fixture.store,
            &hits,
            &[(*first).clone()],
            "model",
            "violet lighthouse",
            &cancel,
        )
        .unwrap();
        assert_eq!(details.len(), 1);
        let detail = &details[&first.path];
        assert_eq!(detail.coverage, "complete");
        assert_eq!(detail.passages.len(), explanation::MAX_PASSAGES);
        assert_eq!(detail.passages[0].location, "passage 0");
        assert_eq!(detail.passages[0].kind, "Keyword match");
        assert!(detail.passages[0].text.contains("Violet lighthouse"));
        assert!(detail.passages[0].text.chars().count() <= 321);
        for keyword in [false, true] {
            hits[1].segments[0].keyword = keyword;
            let details = explanation::read(
                &fixture.store,
                &hits,
                &[(*other).clone()],
                "model",
                "violet",
                &cancel,
            )
            .unwrap();
            assert!(
                details[&other.path].passages.is_empty(),
                "a foreign segment must not be displayed (keyword={keyword})"
            );
        }
        let details = explanation::read(
            &fixture.store,
            &hits,
            &[(*first).clone()],
            "another model",
            "violet",
            &cancel,
        )
        .unwrap();
        assert!(details[&first.path].passages.is_empty());
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(
            explanation::read(
                &fixture.store,
                &hits,
                &[(*first).clone()],
                "model",
                "violet",
                &cancel
            )
            .is_err()
        );
    }

    #[test]
    fn changed_exclusions_filter_cached_rows_before_candidate_caps() {
        let mut fixture = Fixture::new();
        for number in 0..501 {
            fixture.add(
                &format!("ignore/entry_{number}.txt"),
                "old cache",
                Some(1.0),
                2,
            );
        }
        fixture.add("public.txt", "ordinary content", Some(0.8), 2);
        // Populate the vector cache before changing the query's policy. Both
        // accepted and excluded paths have adjacent multiple segment rows.
        let before = fixture.find("meaning", fixture.source.path(), ";scope:recursive", true);
        assert_eq!(before.len(), LIMIT);
        assert!(!before.iter().any(|name| name == "public.txt"));
        fixture
            .exclusions
            .push(fixture.source.path().join("ignore"));
        assert_eq!(
            fixture.find("meaning", fixture.source.path(), ";scope:recursive", true),
            ["public.txt"]
        );
        fixture
            .exclusions
            .push(fixture.source.path().join("public.txt"));
        assert!(
            fixture
                .find("public.txt", fixture.source.path(), ";scope:indexed", true)
                .is_empty()
        );
    }

    #[test]
    fn hidden_index_policy_filters_cached_memberships_even_when_browser_shows_hidden() {
        let mut fixture = Fixture::new();
        fixture.add(".private/needle.txt", "needle", Some(1.0), 1);
        assert_eq!(
            fixture.find("needle", fixture.source.path(), ";scope:indexed", true),
            ["needle.txt"]
        );
        fixture.index_hidden = false;
        for scope in [";scope:indexed", ";scope:recursive"] {
            assert!(
                fixture
                    .find("needle", fixture.source.path(), scope, true)
                    .is_empty()
            );
        }
    }

    #[test]
    fn recursive_and_indexed_visibility_excludes_hidden_ancestors() {
        let mut fixture = Fixture::new();
        fixture.add("visible.txt", "needle", Some(1.0), 1);
        fixture.add(".archive/inside.txt", "needle", Some(1.0), 1);
        let root = fixture.source.path();
        assert_eq!(
            fixture.find("needle", root, ";scope:recursive", false),
            ["visible.txt"]
        );
        assert_eq!(
            fixture.find("needle", root, ";scope:indexed", false),
            ["visible.txt"]
        );
        assert_eq!(
            fixture.find("needle", root, ";scope:recursive", true).len(),
            2
        );
        assert_eq!(
            fixture.find("needle", root, ";scope:indexed", true).len(),
            2
        );
        assert_eq!(
            fixture.find("needle", &root.join(".archive"), ";scope:here", false),
            ["inside.txt"]
        );
    }

    #[test]
    fn hidden_root_visibility_requires_its_verified_membership_and_survives_parent_retirement() {
        let mut fixture = Fixture::new();
        let entry = fixture.add(".archive/inside.txt", "needle", Some(1.0), 1);
        let hidden = fixture.source.path().join(".archive");
        fixture
            .store
            .directory_complete(fixture.root, fixture.generation, &hidden)
            .unwrap();
        let root = Root::open(&hidden).unwrap();
        let nested = fixture.store.enroll(&root).unwrap();
        let generation = fixture.store.begin_scan(nested).unwrap();
        assert!(
            fixture
                .find("needle", fixture.source.path(), ";scope:indexed", false)
                .is_empty()
        );
        // Availability of a containing root alone cannot expose another root's
        // hidden inventory. The path needs that root's actual verified membership.
        fixture
            .store
            .observe(
                nested,
                generation,
                &entry,
                &std::fs::metadata(&entry.path).unwrap(),
                "model",
            )
            .unwrap();
        assert_eq!(
            fixture.find("needle", fixture.source.path(), ";scope:indexed", false),
            ["inside.txt"]
        );
        fixture.store.begin_scan(fixture.root).unwrap();
        fixture.store.retire_path(fixture.root, &hidden).unwrap();
        fixture.store.finish_scan(fixture.root, None).unwrap();
        let parent = fixture
            .store
            .roots()
            .unwrap()
            .into_iter()
            .find(|r| r.id == fixture.root)
            .unwrap();
        assert_eq!(parent.state, "ready");
        assert_eq!(parent.files, 0);
        assert_eq!(
            fixture.find("needle", fixture.source.path(), ";scope:indexed", false),
            ["inside.txt"]
        );
        assert!(
            fixture
                .find("needle", fixture.source.path(), ";scope:recursive", false)
                .is_empty()
        );
        assert_eq!(std::fs::read(&entry.path).unwrap(), b"needle");
        fixture.store.begin_scan(nested).unwrap();
        assert!(
            fixture
                .find("needle", fixture.source.path(), ";scope:indexed", false)
                .is_empty()
        );
    }

    #[test]
    fn unconfigured_cached_roots_cannot_supply_matches_or_scope_after_reopen_or_clear() {
        let mut fixture = Fixture::new();
        fixture.add("one.txt", "needle", Some(1.0), 1);
        assert_eq!(
            fixture.find("needle", fixture.source.path(), "", false),
            ["one.txt"]
        );
        fixture.store.disable_unconfigured(&[]).unwrap();
        assert!(
            fixture
                .find("needle", fixture.source.path(), "", false)
                .is_empty()
        );
        assert!(fixture.store.roots().unwrap().is_empty());
        assert!(fixture.store.source_roots().unwrap().is_empty());
        let count: u64 = fixture
            .store
            .connection
            .query_row("SELECT COUNT(*) FROM segments", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            count, 1,
            "Removing configuration must retain generated work without authorizing it"
        );
        fixture
            .store
            .unavailable(fixture.source.path(), "still unavailable")
            .unwrap();
        assert!(fixture.store.roots().unwrap().is_empty());
        fixture
            .store
            .disable_unconfigured(&[fixture.source.path().into()])
            .unwrap();
        assert_eq!(fixture.store.roots().unwrap().len(), 1);
        assert!(
            fixture.store.source_roots().unwrap().is_empty(),
            "Configured intent must still pass fresh source admission"
        );
        fixture.store.disable_unconfigured(&[]).unwrap();
        fixture.store.clear_generated().unwrap();
        assert!(
            fixture.store.roots().unwrap().is_empty(),
            "Clear must not reactivate cached enrollments"
        );
        assert!(fixture.store.source_roots().unwrap().is_empty());
    }

    #[test]
    fn exact_filenames_outrank_meaning_even_for_name_only_files() {
        let mut fixture = Fixture::new();
        fixture.add("report.rs", "code is name-only", None, 0);
        fixture.add("REPORT.RS", "other code", None, 0);
        fixture.add("word.txt", "report rs", Some(1.0), 1);
        assert_eq!(
            fixture.find("report.rs", fixture.source.path(), "", false),
            ["report.rs", "REPORT.RS", "word.txt"]
        );
    }
    #[test]
    fn no_summing_of_segments_and_low_semantic_scores_do_not_make_results() {
        let mut fixture = Fixture::new();
        fixture.add("short.txt", "unrelated literal text", Some(0.8), 1);
        fixture.add("long.txt", "unrelated literal text", Some(0.6), 40);
        fixture.add("weak.txt", "unrelated literal text", Some(0.1), 1);
        assert_eq!(
            fixture.find("different", fixture.source.path(), "", false),
            ["short.txt", "long.txt"]
        );
    }
    #[test]
    fn lexical_or_fallback_is_scoped_and_hidden_and_stale_sources_are_excluded() {
        let mut fixture = Fixture::new();
        fixture.add("outside.txt", "red missing", Some(0.0), 1);
        fixture.add("inner/red.txt", "red", Some(0.0), 1);
        fixture.add("inner/.red.txt", "red missing", Some(1.0), 1);
        let scope = fixture.source.path().join("inner");
        assert_eq!(fixture.find("red missing", &scope, "", false), ["red.txt"]);
        assert_eq!(fixture.find("red missing", &scope, "", true), [".red.txt"]);
        fixture
            .store
            .invalidate_directory(fixture.root, Some(&scope))
            .unwrap();
        assert!(fixture.find("red missing", &scope, "", false).is_empty());
    }
    #[test]
    fn hard_link_visibility_comes_from_each_path_not_shared_object_metadata() {
        let mut fixture = Fixture::new();
        fixture.add("visible.txt", "needle", Some(1.0), 1);
        std::fs::hard_link(
            fixture.source.path().join("visible.txt"),
            fixture.source.path().join(".hidden.txt"),
        )
        .unwrap();
        fixture.add(".hidden.txt", "needle", Some(1.0), 1);
        assert_eq!(
            fixture.find("needle", fixture.source.path(), "", false),
            ["visible.txt"]
        );
        assert_eq!(
            fixture
                .find("needle", fixture.source.path(), "", true)
                .len(),
            2
        );
    }

    #[test]
    fn hard_filters_run_before_each_component_limit() {
        let mut fixture = Fixture::new();
        for i in 0..=LIMIT {
            fixture.add(&format!("tiny{i}.txt"), "needle", Some(1.0), 1);
        }
        fixture.add("large.txt", &"needle ".repeat(200), Some(0.4), 1);
        assert_eq!(
            fixture.find("needle", fixture.source.path(), ">=1KB", false),
            ["large.txt"]
        );
    }

    #[test]
    fn keyword_window_preserves_natural_ties_after_exclusions_and_unverified_memberships() {
        let mut fixture = Fixture::new();
        // Reverse insertion order and repeated passages must not determine
        // ties, displace eligible files, or reward files with more segments.
        for index in (0..LIMIT + 32).rev() {
            fixture.add(
                format!("item{index}.txt"),
                "needle",
                Some(0.0),
                1 + index % 3,
            );
        }
        for index in 0..10 {
            fixture
                .exclusions
                .push(fixture.source.path().join(format!("item{index}.txt")));
        }
        for index in 10..20 {
            let path = fixture.source.path().join(format!("item{index}.txt"));
            fixture
                .store
                .connection
                .execute(
                    "UPDATE memberships SET verified=0 WHERE path=?1",
                    [path.as_os_str().as_encoded_bytes()],
                )
                .unwrap();
        }
        let expected = (20..20 + LIMIT)
            .map(|index| format!("item{index}.txt"))
            .collect::<Vec<_>>();
        assert_eq!(
            fixture.find("needle", fixture.source.path(), "", false),
            expected
        );
    }
    #[test]
    fn combining_marks_and_literals_use_safe_fts_tokens() {
        let mut fixture = Fixture::new();
        fixture.add("words.txt", "café किताब", Some(0.0), 1);
        assert_eq!(
            fixture.find("cafe", fixture.source.path(), "", false),
            ["words.txt"]
        );
        assert_eq!(
            fixture.find("किताब", fixture.source.path(), "", false),
            ["words.txt"]
        );
        assert_eq!(
            terms("foo\" OR text:bar*").unwrap(),
            ["\"foo\"", "\"OR\"", "\"text\"", "\"bar\"*"]
        );
    }
    #[test]
    fn component_storage_stays_bounded_when_files_gain_better_segments() {
        let mut fixture = Fixture::new();
        let base = fixture.add("base.txt", "text", None, 0);
        let mut top = Top::default();
        for i in 0..=LIMIT {
            let mut entry = (*base).clone();
            entry.name = format!("file{i}.txt");
            entry.path = fixture.source.path().join(&entry.name);
            top.offer(Ranked {
                entry: Arc::new(entry),
                tier: 0,
                score: i as f64,
                segments: Vec::new(),
            });
        }
        assert_eq!(top.by_path.len(), LIMIT);
        assert_eq!(top.ordered.len(), LIMIT);
        let mut improved = top.best().last().unwrap().clone();
        improved.score = 10000.0;
        top.offer(improved.clone());
        assert_eq!(top.best().next().unwrap().entry.path, improved.entry.path);
        assert_eq!(top.by_path.len(), LIMIT);
        assert_eq!(top.ordered.len(), LIMIT);
    }

    #[test]
    fn modality_count_cannot_overpower_the_lexical_weight() {
        let mut fixture = Fixture::new();
        let keyword = fixture.add("keyword.txt", "text", None, 0);
        let meaning = fixture.add("meaning.txt", "text", None, 0);
        let mut lexical = Top::default();
        lexical.offer(Ranked {
            entry: keyword,
            tier: 3,
            score: 1.0,
            segments: Vec::new(),
        });
        let mut modalities = BTreeMap::new();
        for name in ["text", "image", "audio"] {
            let mut list = Top::default();
            list.offer(Ranked {
                entry: meaning.clone(),
                tier: 0,
                score: 1.0,
                segments: Vec::new(),
            });
            modalities.insert(name.into(), list);
        }
        let names = fuse(&lexical, &modalities, "different", false)
            .into_iter()
            .map(|hit| hit.entry.name)
            .collect::<Vec<_>>();
        assert_eq!(names, ["keyword.txt", "meaning.txt"]);
        // A partial match must remain weaker even when only one of several
        // modalities supports the semantic candidate.
        modalities.get_mut("text").unwrap().by_path.clear();
        modalities.get_mut("text").unwrap().ordered.clear();
        modalities.get_mut("image").unwrap().by_path.clear();
        modalities.get_mut("image").unwrap().ordered.clear();
        let names = fuse(&lexical, &modalities, "different", true)
            .into_iter()
            .map(|hit| hit.entry.name)
            .collect::<Vec<_>>();
        assert_eq!(names, ["meaning.txt", "keyword.txt"]);
    }
}
