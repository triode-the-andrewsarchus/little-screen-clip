//! Bounded on-demand filename/content traversal; never enrolls or embeds files.
use super::{
    content::{self, Literal},
    query::{Operator, Query, Scope},
    reader::{self, Reply, Request, SourceRoot},
    store::Store,
};
use crate::file_manager::filesystem::{Directory, Entry};
use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub(super) enum ScopeRoot {
    Folder(PathBuf),
    Indexed(SourceRoot),
}

struct Scan<'a> {
    query: &'a Query,
    cache: &'a Path,
    store: Option<&'a Store>,
    reused: usize,
    exclusions: &'a [PathBuf],
    hidden: bool,
    cancel: &'a Arc<AtomicBool>,
    entries: Vec<Entry>,
    explanations: super::explanation::Explanations,
    sources: Vec<SourceRoot>,
    incomplete: usize,
    seen: HashSet<PathBuf>,
    visited: HashSet<PathBuf>,
    started: Instant,
    limit: &'static str,
}

pub(super) fn run(
    roots: Vec<ScopeRoot>,
    cache: &Path,
    store: Option<&Store>,
    exclusions: &[PathBuf],
    query: &Query,
    hidden: bool,
    cancel: &Arc<AtomicBool>,
) -> Result<super::Results, String> {
    let mut scan = Scan {
        query,
        cache,
        store,
        reused: 0,
        exclusions,
        hidden,
        cancel,
        entries: Vec::new(),
        explanations: Default::default(),
        sources: Vec::new(),
        incomplete: 0,
        seen: HashSet::new(),
        visited: HashSet::new(),
        started: Instant::now(),
        limit: "",
    };
    for origin in roots {
        scan.check_cancel()?;
        match origin {
            ScopeRoot::Indexed(root) => scan.root(root)?,
            ScopeRoot::Folder(path) if super::policy::excluded_by_user(&path, exclusions) => {
                continue;
            }
            ScopeRoot::Folder(path) => match reader::call(&Request::Probe(path), cancel) {
                Ok(Reply::Root(root)) => scan.root(root)?,
                _ => scan.incomplete += 1,
            },
        }
        if !scan.limit.is_empty() {
            break;
        }
    }
    scan.check_cancel()?;
    let stale = scan.verify_content()?;
    scan.entries
        .sort_by_key(|entry| query.local_rank(entry).unwrap_or((u8::MAX, usize::MAX)));
    let mut message = format!(
        "{} · {} matches · {} unsupported/incomplete reads{}",
        if query.progressive {
            "Automatic subtree / configured locations"
        } else {
            "Explicit search scope"
        },
        scan.entries.len(),
        scan.incomplete,
        scan.limit
    );
    if stale > 0 {
        message.push_str(&format!(" · {stale} stale/unavailable omitted"));
    }
    if scan.reused > 0 {
        message.push_str(&format!(
            " · indexed text reused: {} file{}",
            scan.reused,
            if scan.reused == 1 { "" } else { "s" }
        ));
    }
    Ok(super::Results {
        entries: scan.entries,
        message,
        sources: scan.sources,
        explanations: scan.explanations,
    })
}

impl Scan<'_> {
    fn check_cancel(&self) -> Result<(), String> {
        if self.cancel.load(Ordering::Relaxed) {
            Err("Search cancelled".into())
        } else {
            Ok(())
        }
    }

    fn verify_content(&mut self) -> Result<usize, String> {
        if !matches!(self.query.operator, Operator::Content(_)) {
            return Ok(0);
        }
        let verified = reader::verification::current(&self.entries, &self.sources, self.cancel)?;
        let stale = self.entries.len() - verified.len();
        self.entries.retain(|entry| verified.contains(&entry.path));
        self.explanations.retain(|path, _| verified.contains(path));
        self.incomplete += stale;
        Ok(stale)
    }

    fn root(&mut self, root: SourceRoot) -> Result<(), String> {
        if super::policy::excluded_by_user(&root.path, self.exclusions) {
            return Ok(());
        }
        self.sources.push(root.clone());
        let mut directories = VecDeque::from([root.path.clone()]);
        while let Some(path) = directories.pop_front() {
            self.check_cancel()?;
            if !self.visited.insert(path.clone()) {
                continue;
            }
            if self.visited.len() > 10_000
                || (self.query.progressive && self.started.elapsed() > Duration::from_secs(30))
            {
                self.limit = " · partial: directory/time budget";
                break;
            }
            match reader::call(
                &Request::List {
                    output: None,
                    root: root.clone(),
                    path,
                    hidden: self.hidden,
                    exclusions: self.exclusions.to_vec(),
                },
                self.cancel,
            ) {
                Ok(Reply::Directory(inventory)) => {
                    self.directory(&root, inventory.directory, &mut directories)?
                }
                _ => self.incomplete += 1,
            }
            if !self.limit.is_empty() {
                break;
            }
        }
        Ok(())
    }

    fn match_entry(&mut self, root: &SourceRoot, entry: &Entry) -> Result<content::Match, String> {
        // Cache failure is a miss: ordinary literal search remains
        // available even when its optional derived index is unreadable.
        if let (Operator::Content(needle), Some(store)) = (&self.query.operator, self.store)
            && self.query.local_rank(entry).is_some()
            && reader::supports_text(entry)
            && let Some(result) = store
                .cached_content(root, entry, needle, self.cancel)
                .ok()
                .flatten()
        {
            self.reused += 1;
            return Ok(result);
        }
        matches_query(root, entry, self.query, self.cancel)
    }

    fn directory(
        &mut self,
        root: &SourceRoot,
        directory: Directory,
        directories: &mut VecDeque<PathBuf>,
    ) -> Result<(), String> {
        self.incomplete += directory.skipped;
        for entry in directory.entries {
            self.check_cancel()?;
            if entry.path.starts_with(self.cache) || !self.seen.insert(entry.path.clone()) {
                continue;
            }
            if self.seen.len() > 100_000 {
                self.limit = " · partial: 100,000 visited-entry limit";
                break;
            }
            if self.query.scope != Scope::Here && entry.directory {
                if entry
                    .path
                    .components()
                    .count()
                    .saturating_sub(root.path.components().count())
                    < 64
                {
                    directories.push_back(entry.path.clone());
                } else {
                    self.incomplete += 1;
                }
            }
            match self.match_entry(root, &entry) {
                Ok((matched, partial, explanation)) => {
                    self.incomplete += usize::from(partial);
                    if matched {
                        if let Some(explanation) = explanation {
                            self.explanations.insert(entry.path.clone(), explanation);
                        }
                        self.entries.push(entry);
                    }
                }
                Err(_) => self.incomplete += 1,
            }
        }
        Ok(())
    }
}

fn matches_query(
    root: &SourceRoot,
    entry: &Entry,
    query: &Query,
    stop: &AtomicBool,
) -> Result<content::Match, String> {
    if query.local_rank(entry).is_none() {
        return Ok((false, false, None));
    }
    match &query.operator {
        Operator::Content(needle) if entry.regular_file => content_match(root, entry, needle, stop),
        Operator::Content(_) => Ok((false, false, None)),
        _ => Ok((true, false, None)),
    }
}

fn content_match(
    root: &SourceRoot,
    entry: &Entry,
    needle: &str,
    stop: &AtomicBool,
) -> Result<content::Match, String> {
    if !reader::supports_text(entry) {
        return Ok((false, true, None));
    }
    let mut literal = Literal::new(needle);
    let mut cursor = 0;
    let mut partial = false;
    loop {
        let Reply::Prepared(prepared) = reader::call(
            &Request::Extract {
                root: root.clone(),
                entry: entry.clone(),
                cursor,
                text_only: true,
            },
            stop,
        )?
        else {
            return Err("Invalid content extraction reply".into());
        };
        partial |= prepared.coverage != "complete" || prepared.data.is_some();
        let coverage = if prepared.next.is_some() {
            format!("{}; remaining content not scanned", prepared.coverage)
        } else {
            prepared.coverage
        };
        if let Some(explanation) = literal.push(&prepared.text, &prepared.location, coverage) {
            return Ok((true, partial, Some(explanation)));
        }
        match prepared.next {
            Some(next) => cursor = next,
            None => return Ok((false, partial, None)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_search_does_not_spawn_extraction_for_unsupported_media() {
        let folder = tempfile::tempdir().unwrap();
        for name in ["image.png", "sound.wav", "movie.mp4"] {
            std::fs::write(folder.path().join(name), b"not valid media").unwrap();
        }
        let origin = super::super::policy::Root::open(folder.path()).unwrap();
        let mut root = SourceRoot::from_root(&origin);
        // Any helper request would reject this missing mount observation. A
        // supported no-work result proves media was skipped before dispatch.
        root.mount = None;
        let directory =
            crate::file_manager::filesystem::read_directory(folder.path(), &AtomicBool::new(false))
                .unwrap();
        let query = Query::parse(":content:\"invoice number\"").unwrap();
        for entry in directory.entries {
            let (matched, partial, explanation) =
                matches_query(&root, &entry, &query, &AtomicBool::new(false)).unwrap();
            assert!(!matched && partial && explanation.is_none());
        }
    }
}
