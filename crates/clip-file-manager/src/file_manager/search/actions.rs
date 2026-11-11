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
    /// Retain only the observations selected by an action, without re-enumeration.
    pub fn guards_for(&self, paths: &[PathBuf]) -> Result<HashMap<PathBuf, SourceGuard>, String> {
        if !self.active() {
            return Ok(HashMap::new());
        }
        paths
            .iter()
            .map(|path| {
                self.guards
                    .get(path)
                    .cloned()
                    .map(|guard| (path.clone(), guard))
                    .ok_or_else(|| "Action is outside the published search results".into())
            })
            .collect()
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
    pub fn expected_identity(&self) -> Option<filesystem::Identity> {
        self.entry.identity
    }
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
    pub(in crate::file_manager) fn search_preview_context(&self) -> PreviewContext {
        self.search.preview_context.clone()
    }
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
        self.gallery_hover.set_context(context);
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
    fn confirmed_search_deletion_keeps_original_root_even_if_leaf_identity_is_unchanged() {
        use std::os::unix::fs::symlink;
        use std::time::{Duration, Instant};
        for searched in [false, true] {
            let fixture = tempfile::tempdir().unwrap();
            let root = fixture.path().join("source");
            let original = fixture.path().join("original");
            std::fs::create_dir(&root).unwrap();
            let path = root.join("one.txt");
            std::fs::write(&path, b"original bytes").unwrap();
            let observed = super::super::policy::Root::open(&root).unwrap();
            let directory = filesystem::read_directory(&root, &AtomicBool::new(false)).unwrap();
            let expected = directory.entries[0].identity;
            let mut manager = FileManager::new(root.clone());
            manager.pending = None;
            manager.commit(directory, crate::file_manager::Navigation::New);
            if searched {
                manager.search.sources = Some(Arc::new(vec![SourceRoot::from_root(&observed)]));
            }
            manager.select(path.clone());
            manager.confirm_delete(true);
            std::fs::rename(&root, &original).unwrap();
            symlink(&original, &root).unwrap();
            assert_eq!(
                Some(filesystem::identity(
                    &std::fs::symlink_metadata(&path).unwrap()
                )),
                expected
            );
            let ctx = egui::Context::default();
            let palette =
                crate::ui_appearance::palette(little_clip_core::config::WorkbenchAppearance::Dark);
            let _ = ctx.run(
                egui::RawInput {
                    events: vec![egui::Event::Key {
                        key: egui::Key::Enter,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }],
                    ..Default::default()
                },
                |ctx| manager.transfer_windows(ctx, &palette),
            );
            assert!(
                manager.transfers.busy(),
                "confirmation must enqueue the batch"
            );
            let jobs = little_clip_core::BackgroundJobs::new(2, 16);
            let deadline = Instant::now() + Duration::from_secs(5);
            while manager.transfers.busy() {
                manager.tick_transfers(&jobs);
                assert!(Instant::now() < deadline, "delete batch did not finish");
                std::thread::sleep(Duration::from_millis(2));
            }
            assert_eq!(
                original.join("one.txt").exists(),
                searched,
                "search actions must reject an aliased root without changing ordinary alias handling"
            );
            if searched {
                assert_eq!(
                    std::fs::read(original.join("one.txt")).unwrap(),
                    b"original bytes"
                );
            }
        }
    }

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
        assert!(
            context
                .guards_for(&[PathBuf::from("/unpublished")])
                .is_err()
        );
        let retained = context.clone();
        assert!(context.replace(&PreviewContext::default()));
        assert!(
            retained
                .guards_for(&[PathBuf::from("/unpublished")])
                .is_err()
        );
        assert!(
            context
                .guards_for(&[PathBuf::from("/ordinary")])
                .unwrap()
                .is_empty()
        );
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
        let path = offline.entry.path.clone();
        let expected = offline.expected_identity().unwrap();
        let mut manager = FileManager::new(directory.path().into());
        manager.pending = None;
        manager.commit(
            filesystem::read_directory(directory.path(), &AtomicBool::new(false)).unwrap(),
            crate::file_manager::Navigation::New,
        );
        for cut in [true, false] {
            manager.shelf.add(
                crate::file_manager::content::Payload::Files {
                    paths: vec![path.clone()],
                    cut,
                },
                "chosen reference".into(),
            );
        }
        let jobs = little_clip_core::BackgroundJobs::new(2, 16);
        // Completion belongs to the original drag, not the current browser
        // query. A receiver that already removed it needs only a no-op ack.
        for externally_completed in [false, true] {
            if externally_completed {
                std::fs::remove_file(&path).unwrap();
            }
            manager.native_move_cleanup(
                std::collections::BTreeMap::from([(path.clone(), Some(expected))]),
                std::collections::BTreeMap::from([(path.clone(), expected)]),
                HashMap::from([(path.clone(), offline.clone())]),
            );
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while manager.transfers.busy() {
                manager.tick_transfers(&jobs);
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            if externally_completed {
                assert_eq!(
                    manager.shelf.len(),
                    1,
                    "completed moves consume only cut references"
                );
            } else {
                assert_eq!(std::fs::read(&path).unwrap(), b"original content");
                assert_eq!(
                    manager.shelf.len(),
                    2,
                    "rejected moves cannot consume references"
                );
            }
        }
    }
}
