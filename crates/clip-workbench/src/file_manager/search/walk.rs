//! Bounded on-demand filename/content traversal; never enrolls or embeds files.
use super::{
    query::{Operator, Query, Scope},
    reader::{self, Reply, Request, SourceRoot},
};
use crate::file_manager::filesystem::{Directory, Entry};
use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub(super) enum ScopeRoot {
    Folder(PathBuf),
    Indexed(SourceRoot),
}

struct Scan<'a> {
    query: &'a Query,
    cache: &'a Path,
    exclusions: &'a [PathBuf],
    hidden: bool,
    cancel: &'a AtomicBool,
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
    exclusions: &[PathBuf],
    query: &Query,
    hidden: bool,
    cancel: &AtomicBool,
) -> Result<super::Results, String> {
    let mut scan = Scan {
        query,
        cache,
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
    scan.entries
        .sort_by_key(|entry| query.local_rank(entry).unwrap_or((u8::MAX, usize::MAX)));
    let message = format!(
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
                Ok(Reply::Directory(directory)) => {
                    self.directory(&root, directory, &mut directories)?
                }
                _ => self.incomplete += 1,
            }
            if !self.limit.is_empty() {
                break;
            }
        }
        Ok(())
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
            match matches_query(root, &entry, self.query, self.cancel) {
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
) -> Result<(bool, bool, Option<super::explanation::Explanation>), String> {
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
) -> Result<(bool, bool, Option<super::explanation::Explanation>), String> {
    let needle = normalize(needle);
    let mut cursor = 0;
    let mut tail = String::new();
    let mut partial = false;
    loop {
        let Reply::Prepared(prepared) = reader::call(
            &Request::Extract {
                root: root.clone(),
                entry: entry.clone(),
                cursor,
            },
            stop,
        )?
        else {
            return Err("Invalid content extraction reply".into());
        };
        partial |= prepared.coverage != "complete" || prepared.data.is_some();
        let preceding = tail.to_lowercase().len();
        if !tail.is_empty() {
            tail.push(' ');
        }
        tail.push_str(
            &prepared
                .text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        );
        if let Some(position) = tail.to_lowercase().find(&needle) {
            let location = if position < preceding {
                format!("Across extraction chunks, ending at {}", prepared.location)
            } else {
                prepared.location.clone()
            };
            let coverage = if prepared.next.is_some() {
                format!("{}; remaining content not scanned", prepared.coverage)
            } else {
                prepared.coverage
            };
            let explanation = super::explanation::Explanation {
                unavailable: false,
                coverage,
                passages: vec![super::explanation::Passage {
                    kind: "Content match".into(),
                    location,
                    text: super::explanation::excerpt(&tail, &needle),
                }],
            };
            return Ok((true, partial, Some(explanation)));
        }
        tail = tail
            .chars()
            .rev()
            .take(needle.chars().count())
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        match prepared.next {
            Some(next) => cursor = next,
            None => return Ok((false, partial, None)),
        }
    }
}
fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
