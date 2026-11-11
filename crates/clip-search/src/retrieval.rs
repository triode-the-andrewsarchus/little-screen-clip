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
use little_clip_core::{file_inventory::Entry, natural_sort::natural_cmp};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering},
    },
};

pub const LIMIT: usize = 500;
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
/// A ranked observation with bounded supporting segment matches.
pub struct Hit {
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
        let name = little_clip_core::path_address::display_path_cow(Path::new(
            path.file_name().unwrap_or_default(),
        ));
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

/// Scoped SQLite cancellation for one read on a coordinator-owned connection.
///
/// Covers prepare/step and removes the callback on drop before later writes.
/// Do not nest guards or share the connection with another request owner.
pub struct SqlCancellation<'a>(&'a rusqlite::Connection);
impl<'a> SqlCancellation<'a> {
    pub fn new(connection: &'a rusqlite::Connection, cancel: &Arc<AtomicBool>) -> Self {
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

/// One hard-filtered lexical/vector request with bounded rank fusion.
///
/// Offline observations remain labeled; callers revalidate sources before opening.
pub struct Retrieval<'a> {
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
    /// Retrieves bounded ranked candidates under the existing filters and budgets.
    ///
    /// # Errors
    /// Returns cancellation, invalid input, vector or SQLite failures.
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
                let name = little_clip_core::path_address::display_path_cow(Path::new(
                    path.file_name().unwrap_or_default(),
                ));
                Ok(crate::name_match::name_rank(&name, query).is_some())
            },
        )
        .map_err(|e| e.to_string())
}

pub fn terms(text: &str) -> Result<Vec<String>, String> {
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
        entry.name = little_clip_core::path_address::display_path(Path::new(
            current.file_name().unwrap_or_default(),
        ));
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
        Ok(crate::name_match::name_rank(name, folded)
            .map(|tier| (tier, if name == self.text { 1.0 } else { 0.0 })))
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
            let name = little_clip_core::path_address::display_path_cow(Path::new(
                path.file_name().unwrap_or_default(),
            ));
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
        let name = little_clip_core::path_address::display_path_cow(Path::new(
            path.file_name().unwrap_or_default(),
        ));
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
mod tests;
