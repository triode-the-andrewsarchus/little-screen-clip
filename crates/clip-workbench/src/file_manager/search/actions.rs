//! Search actions retain the observed file and original source identities.
use super::{
    FileManager,
    reader::{self, Reply, Request, SourceRoot},
};
use crate::file_manager::filesystem::{self, Directory, Entry};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};

/// One published result set. Clones share source expectations and observations.
#[derive(Clone, Default)]
pub(in crate::file_manager) struct PreviewContext {
    sources: Option<Arc<Vec<SourceRoot>>>,
    guards: Arc<HashMap<PathBuf, SourceGuard>>,
}
impl PreviewContext {
    #[cfg(test)]
    pub fn empty_results() -> Self {
        Self {
            sources: Some(Arc::new(Vec::new())),
            ..Self::default()
        }
    }
    fn matches(&self, sources: &Option<Arc<Vec<SourceRoot>>>) -> bool {
        match (&self.sources, sources) {
            (Some(left), Some(right)) => Arc::ptr_eq(left, right),
            (None, None) => true,
            _ => false,
        }
    }
    pub fn same(&self, other: &Self) -> bool {
        self.matches(&other.sources)
    }
    /// Report a generation transition once; caches retire their owned jobs then.
    pub fn replace(&mut self, next: &Self) -> bool {
        if self.same(next) {
            false
        } else {
            *self = next.clone();
            true
        }
    }
    pub fn active(&self) -> bool {
        self.sources.is_some()
    }
    pub fn read<T>(
        &self,
        path: &Path,
        cancel: &AtomicBool,
        read: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let guard = if self.active() {
            Some(
                self.guards
                    .get(path)
                    .ok_or("Preview is outside the published search results")?,
            )
        } else {
            None
        };
        SourceGuard::read_checked(guard, cancel, read)
    }
}

#[derive(Clone)]
pub(in crate::file_manager) struct SourceGuard {
    entry: Entry,
    sources: Arc<Vec<SourceRoot>>,
    unavailable: bool,
}
impl SourceGuard {
    pub fn read_check(&self) -> crate::file_source::ReadCheck {
        let guard = self.clone();
        crate::file_source::ReadCheck::new(move |cancel| guard.check(cancel))
    }
    pub fn same_observation(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.sources, &other.sources)
            && self.entry.path == other.entry.path
            && self.entry.identity == other.entry.identity
            && self.entry.revision == other.entry.revision
            && self.unavailable == other.unavailable
    }

    /// Run only on a worker. A fresh probe must never adopt a replacement root.
    pub fn check(&self, cancel: &AtomicBool) -> Result<(), String> {
        if self.unavailable {
            return Err("Last-known unavailable result. Reconnect and refresh the search before opening or collecting it.".into());
        }
        for root in self
            .sources
            .iter()
            .filter(|root| self.entry.path.starts_with(&root.path))
        {
            if matches!(
                reader::call(
                    &Request::Verify {
                        root: root.clone(),
                        entry: self.entry.clone()
                    },
                    cancel
                ),
                Ok(Reply::Verified)
            ) {
                return Ok(());
            }
        }
        Err("Search result changed or its source is unavailable. Refresh the search before opening it.".into())
    }
    /// Validate the observed search context around a worker-side content read.
    pub fn read_checked<T>(
        guard: Option<&Self>,
        cancel: &AtomicBool,
        read: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        guard
            .map(Self::read_check)
            .unwrap_or_default()
            .read(cancel, read)
    }

    pub fn read_directory(&self, folder: &Path, cancel: &AtomicBool) -> Result<Directory, String> {
        if folder != self.entry.path.parent().unwrap_or(Path::new("/"))
            && !(self.entry.directory && folder == self.entry.path)
        {
            return Err("Search action does not target this result's folder".into());
        }
        Self::read_checked(Some(self), cancel, || {
            filesystem::read_directory(folder, cancel)
        })
    }
}
impl FileManager {
    pub(in crate::file_manager) fn sync_preview_context(&mut self) {
        if !self.search.preview_context.matches(&self.search.sources) {
            let paths = self
                .directory
                .as_ref()
                .map(|directory| {
                    directory
                        .entries
                        .iter()
                        .map(|entry| entry.path.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            self.stop_previews();
            self.midi.selection = None;
            self.music_selection = None;
            self.search.preview_context = PreviewContext {
                sources: self.search.sources.clone(),
                guards: Arc::new(self.search_source_guards(&paths)),
            };
        }
        let context = &self.search.preview_context;
        self.previews.set_context(context);
        self.music_previews.set_context(context);
        self.gallery_previews.set_context(context);
        self.video_previews.set_context(context);
        self.gallery_layout.set_context(context);
    }
    pub(in crate::file_manager) fn take_available_open_file(&mut self) -> Option<PathBuf> {
        let path = self.open_file.take()?;
        if self.search.is_offline(&path) {
            self.error =
                Some("Reconnect and refresh before opening a last-known unavailable file.".into());
            None
        } else {
            Some(path)
        }
    }
    pub(in crate::file_manager) fn search_source_guard(&self, path: &Path) -> Option<SourceGuard> {
        let sources = self.search.sources.as_ref()?;
        let entry = self
            .directory
            .as_ref()?
            .entries
            .iter()
            .find(|entry| entry.path == path)?
            .clone();
        Some(SourceGuard {
            entry,
            sources: sources.clone(),
            unavailable: self.search.is_offline(path),
        })
    }
    pub(in crate::file_manager) fn search_source_guards(
        &self,
        paths: &[PathBuf],
    ) -> std::collections::HashMap<PathBuf, SourceGuard> {
        let (Some(sources), Some(directory)) = (&self.search.sources, &self.directory) else {
            return Default::default();
        };
        let chosen = paths.iter().collect::<std::collections::HashSet<_>>();
        directory
            .entries
            .iter()
            .filter(|entry| chosen.contains(&entry.path))
            .map(|entry| {
                (
                    entry.path.clone(),
                    SourceGuard {
                        entry: entry.clone(),
                        sources: sources.clone(),
                        unavailable: self.search.is_offline(&entry.path),
                    },
                )
            })
            .collect()
    }

    pub(in crate::file_manager) fn reveal_search_result(
        &mut self,
        path: PathBuf,
        open_folder: bool,
    ) {
        let Some(guard) = self.search_source_guard(&path) else {
            return;
        };
        let folder = if open_folder {
            path.clone()
        } else {
            let Some(parent) = path.parent() else { return };
            parent.to_path_buf()
        };
        // Refresh admission is deliberate even when revealing in the same folder.
        let navigation = if folder == self.path() {
            super::super::Navigation::Refresh
        } else {
            super::super::Navigation::New
        };
        self.navigate(folder, super::super::Navigation::Refresh);
        if let Some(request) = &mut self.pending {
            request.navigation = navigation;
            request.search_target = Some(guard);
            self.reveal_target = (!open_folder).then_some(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preview_context_rejects_unpublished_paths_before_reading_and_tracks_generations() {
        let mut context = PreviewContext::default();
        let results = PreviewContext::empty_results();
        assert!(context.replace(&results));
        assert!(!context.replace(&results.clone()));
        let read = std::cell::Cell::new(false);
        assert!(
            context
                .read(Path::new("/unpublished"), &AtomicBool::new(false), || {
                    read.set(true);
                    Ok(())
                })
                .is_err()
        );
        assert!(!read.get());
        assert!(context.replace(&PreviewContext::default()));
        context
            .read(Path::new("/ordinary"), &AtomicBool::new(false), || {
                read.set(true);
                Ok(())
            })
            .unwrap();
        assert!(read.get());
    }
    #[test]
    fn last_known_results_stay_unavailable_until_a_new_search_publication() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("one.txt"), "original content").unwrap();
        let observed = super::super::policy::Root::open(directory.path()).unwrap();
        let root = SourceRoot {
            path: observed.path,
            device: observed.device,
            inode: observed.inode,
            mount: Some(observed.mount),
            session: None,
        };
        let entry = filesystem::read_directory(directory.path(), &AtomicBool::new(false))
            .unwrap()
            .entries
            .remove(0);
        let offline = SourceGuard {
            entry,
            sources: Arc::new(vec![root]),
            unavailable: true,
        };
        assert!(
            offline
                .check(&AtomicBool::new(false))
                .unwrap_err()
                .contains("Reconnect and refresh")
        );
        let mut changed = offline.clone();
        changed.unavailable = false;
        assert!(!offline.same_observation(&changed));
    }
}
