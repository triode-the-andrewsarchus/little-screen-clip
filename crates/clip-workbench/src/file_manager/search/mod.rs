//! Explicit browser searches share one parsed query and background-only admission.
mod actions;
mod answer;
mod columns;
mod columns_ui;
mod controls;
mod explanation;
mod explanation_ui;
mod image_order;
mod monitor;
mod policy;
pub(super) mod query;
mod range;
mod reader;
mod retrieval;
mod runtime;
mod service;
mod session;
mod settings;
mod store;
mod ui;
mod walk;

pub(crate) use answer::extraction::run as run_extraction;
pub(crate) use reader::run as run_reader;

use super::{FileManager, filesystem::Directory};
use query::{Operator, Query, Scope, SortField};

pub(super) use actions::{PreviewContext, SourceGuard};

#[derive(Clone)]
struct Results {
    entries: Vec<super::filesystem::Entry>,
    message: String,
    sources: Vec<reader::SourceRoot>,
    explanations: explanation::Explanations,
}

#[derive(Default)]
pub(super) struct Search {
    pub open: bool,
    pub automatic: bool,
    pub literal: bool,
    controls: controls::Controls,
    pub edit_at: Option<f64>,
    pub draft: String,
    pub focus: bool,
    pub error: Option<String>,
    pub applied: Option<Query>,
    restore: Option<super::Location>,
    restore_sort: Option<super::sorting::Sort>,
    restore_directory: Option<Directory>,
    pub service: Option<service::Service>,
    pub settings: little_clip_core::config::FileSearchConfig,
    pub preferences: settings::Settings,
    request_id: u64,
    pending: bool,
    submitted_at: Option<std::time::Instant>,
    query_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub message: String,
    result_order: std::collections::HashMap<std::path::PathBuf, usize>,
    result_pages: usize,
    sources: Option<std::sync::Arc<Vec<reader::SourceRoot>>>,
    preview_context: PreviewContext,
    explanations: explanation::Explanations,
    explanation_id: Option<u64>,
    matching_count: usize,
    pub images: image_order::ImageOrder,
    pub answers: columns::Columns,
}
impl Search {
    pub(in crate::file_manager) fn is_offline(&self, path: &std::path::Path) -> bool {
        self.sources.is_some()
            && self
                .explanations
                .get(path)
                .is_some_and(|detail| detail.unavailable)
    }
    pub(in crate::file_manager) fn has_offline_results(&self) -> bool {
        self.sources.is_some() && self.explanations.values().any(|detail| detail.unavailable)
    }
    pub(super) fn has_result_sources(&self) -> bool {
        self.sources.is_some()
    }

    pub(super) fn open(&mut self, draft: String) {
        self.open = true;
        self.automatic = false;
        self.literal = false;
        self.edit_at = None;
        self.focus = true;
        self.draft = draft;
        self.error = None;
    }
    fn parse_draft(&self) -> Result<Query, String> {
        if self.automatic {
            Ok(Query::navigation(self.draft.clone()))
        } else if self.literal {
            Query::literal(&self.draft)
        } else {
            Query::parse(&self.draft)
        }
    }
    fn edited(&mut self, now: f64) {
        self.edit_at = Some(now + 0.25);
        self.error = None;
        self.request_id = self.request_id.wrapping_add(1);
        self.pending = false;
        self.submitted_at = None;
        if let Some(cancel) = self.query_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn live_submission(&mut self, ctx: &egui::Context) -> bool {
        let Some(deadline) = self.edit_at else {
            return false;
        };
        let delay = deadline - ctx.input(|input| input.time);
        if delay > 0.0 {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(delay));
            return false;
        }
        self.edit_at = None;
        true
    }

    pub(super) fn paginate(&mut self, visible: &mut Vec<usize>) {
        self.matching_count = visible.len();
        if self
            .applied
            .as_ref()
            .is_some_and(|query| matches!(query.operator, Operator::Embedding(_)))
        {
            visible.truncate(self.result_pages.saturating_add(1).saturating_mul(100));
        }
    }

    pub(super) fn filter(&self, directory: &Directory, visible: &mut Vec<usize>) {
        let Some(query) = &self.applied else { return };
        visible.retain(|index| query.local_rank(&directory.entries[*index]).is_some());
        if let Some(sort) = query
            .sort
            .filter(|sort| matches!(sort.field, SortField::Dhash | SortField::Similarity))
        {
            self.images.order(directory, visible, sort.descending);
        } else if let Some(sort) = query.browser_sort() {
            sort.apply(directory, visible);
        } else if !self.result_order.is_empty() {
            visible.sort_by_key(|index| {
                self.result_order
                    .get(&directory.entries[*index].path)
                    .copied()
                    .unwrap_or(usize::MAX)
            });
        } else {
            visible.sort_by_key(|index| {
                query
                    .local_rank(&directory.entries[*index])
                    .unwrap_or((u8::MAX, usize::MAX))
            });
        }
    }
}
impl FileManager {
    pub(in crate::file_manager) fn apply_search(&mut self) {
        let result = self.search.parse_draft();
        match result {
            Ok(mut query) => {
                if matches!(query.operator, Operator::Answer(_)) {
                    self.add_answer_column(query);
                    return;
                }
                let source = self
                    .search
                    .restore_directory
                    .as_ref()
                    .or(self.directory.as_ref());
                let Some(directory) = source else {
                    self.search.error = Some("Open a folder before searching".into());
                    return;
                };
                query.resolve_navigation_scope(directory, self.show_hidden);
                let selected = self
                    .location()
                    .map(|location| location.selection.clone())
                    .unwrap_or_default();
                if let Err(error) = self.search.images.begin(&query, directory, &selected) {
                    self.search.error = Some(error);
                    return;
                }
                self.search.answers.cancel_pending();
                if query.sort.is_some()
                    && let Some(folder) = self.search.answers.current_mut()
                {
                    folder.sort = None;
                }
                self.search.request_id = self.search.request_id.wrapping_add(1);
                self.search.pending = false;
                self.search.submitted_at = None;
                if let Some(cancel) = self.search.query_cancel.take() {
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
                self.search.result_order.clear();
                self.search.result_pages = 0;
                if self.search.restore.is_none() {
                    self.search.restore = self.location().cloned();
                    self.search.restore_sort = Some(self.sort);
                }
                if let Some(sort) = query.browser_sort() {
                    self.sort = sort;
                }
                let remote = query.scope != Scope::Here
                    || matches!(
                        query.operator,
                        Operator::Content(_) | Operator::Embedding(_)
                    );
                if remote {
                    if self.search.restore_directory.is_none() {
                        self.search.restore_directory = self.directory.clone();
                    }
                    if let Err(error) = self.submit_search(&query) {
                        self.search.error = Some(error);
                        return;
                    }
                } else {
                    self.search.sources = None;
                    self.search.explanations.clear();
                    self.search.message.clear();
                    if let Some(directory) = self.search.restore_directory.take() {
                        self.directory = Some(directory);
                    }
                }
                self.search.applied = Some(query);
                self.search.error = None;
                self.error = None;
                self.failed = None;
                self.rebuild_visible();
                self.select_navigation_result();
                self.restore_scroll = true;
            }
            Err(error) => self.search.error = Some(error),
        }
    }
    /// Refresh accepted results without adopting an unfinished or invalid draft.
    pub(in crate::file_manager) fn refresh_search_results(&mut self) -> bool {
        let Some(query) = self.search.applied.clone().filter(|query| {
            query.scope != Scope::Here
                || matches!(
                    query.operator,
                    Operator::Content(_) | Operator::Embedding(_)
                )
        }) else {
            return false;
        };
        self.error = None;
        self.failed = None;
        self.search.request_id = self.search.request_id.wrapping_add(1);
        if let Some(cancel) = self.search.query_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.search.pending = false;
        self.search.submitted_at = None;
        if let Err(error) = self.submit_search(&query) {
            self.search.error = Some(error);
        }
        true
    }

    fn submit_search(&mut self, query: &Query) -> Result<(), String> {
        let submitted_at = std::time::Instant::now();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.search.query_cancel = Some(cancel.clone());
        let command = service::Command::Search {
            id: self.search.request_id,
            path: self.path().into(),
            query: query.clone(),
            hidden: self.show_hidden,
            cancel,
        };
        self.search_service()?.send(command)?;
        self.search.pending = true;
        self.search.submitted_at = Some(submitted_at);
        self.search.message = "Searching…".into();
        Ok(())
    }

    fn add_answer_column(&mut self, query: Query) {
        let result = self
            .directory
            .as_ref()
            .ok_or_else(|| "Open a folder before adding an answer column".to_owned())
            .and_then(|directory| self.search.answers.submit(directory, &self.visible, query));
        if result.is_ok() {
            self.set_view(super::BrowserView::List);
        }
        self.search.error = result.err();
    }
    pub(in crate::file_manager) fn close_search(&mut self) {
        if let Some(cancel) = self.search.query_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        let restore = self.search.restore.take();
        if let Some(sort) = self.search.restore_sort.take() {
            self.sort = sort;
        }
        if let Some(directory) = self.search.restore_directory.take() {
            self.directory = Some(directory);
        }
        self.search.images.retire();
        self.search = Search {
            images: std::mem::take(&mut self.search.images),
            answers: std::mem::take(&mut self.search.answers),
            service: self.search.service.take(),
            settings: self.search.settings.clone(),
            preferences: std::mem::take(&mut self.search.preferences),
            request_id: self.search.request_id.wrapping_add(1),
            ..Default::default()
        };
        if let (Some(restore), Some(location)) = (restore, self.location_mut())
            && restore.path == location.path
        {
            *location = restore;
        }
        self.rebuild_visible();
        self.restore_scroll = true;
    }
    pub(in crate::file_manager) fn search_service(&mut self) -> Result<&service::Service, String> {
        if self.search.service.is_none() {
            self.search.service = Some(service::Service::start(
                &self.home,
                self.search.settings.clone(),
            )?);
        }
        Ok(self.search.service.as_ref().unwrap())
    }
    pub(in crate::file_manager) fn tick_search(&mut self, jobs: &little_clip_core::BackgroundJobs) {
        self.tick_answer_columns();
        let clear_results = if let Some(service) = &self.search.service {
            let mut snapshot = service.snapshot.lock().unwrap();
            self.search.settings = snapshot.settings.clone();
            snapshot.clear_results.take()
        } else {
            None
        };
        match clear_results {
            Some(service::ResultReset::Scope) => {
                self.search.answers.invalidate();
                self.stop_previews();
                self.close_search();
            }
            Some(service::ResultReset::Semantic)
                if self
                    .search
                    .applied
                    .as_ref()
                    .is_some_and(|query| matches!(query.operator, Operator::Embedding(_))) =>
            {
                self.close_search();
            }
            _ => {}
        }
        if let Some(directory) = &self.directory {
            let origin = self.search.restore_directory.as_ref().unwrap_or(directory);
            let (changed, error) =
                self.search
                    .images
                    .tick(directory, origin, &self.visible, &self.previews, jobs);
            if let Some(error) = error {
                self.search.error = Some(error);
            }
            if changed {
                self.rebuild_visible();
            }
        }
        let result = self
            .search
            .service
            .as_ref()
            .and_then(|service| service.snapshot.lock().ok()?.result.take());
        let Some(result) = result else { return };
        if result.id != self.search.request_id || !self.search.pending {
            return;
        }
        self.search.pending = !result.complete;
        let succeeded = result.result.is_ok();
        match result.result {
            Ok(Results {
                entries,
                message,
                sources,
                explanations,
            }) => {
                self.search.explanations = explanations;
                self.search.explanation_id = Some(result.id);
                self.search.sources = Some(std::sync::Arc::new(sources));
                if self.search.has_offline_results() {
                    self.search.images.retire();
                }
                self.search.result_order = entries
                    .iter()
                    .enumerate()
                    .map(|(i, entry)| (entry.path.clone(), i))
                    .collect();
                self.search.message = message;
                self.directory = Some(Directory {
                    object: None,
                    mount: self
                        .search
                        .restore_directory
                        .as_ref()
                        .filter(|directory| directory.path == self.path())
                        .and_then(|directory| directory.mount),
                    entries,
                    system: false,
                    path: self.path().into(),
                    skipped: 0,
                    signature: 0,
                });
                self.rebuild_visible();
                self.select_navigation_result();
                self.restore_scroll = true;
            }
            Err(error) => self.search.error = Some(error),
        }
        // Only diagnostic counts/timings: never log query text, answers or paths.
        // Measure through UI acceptance and sorting, not GPU frame presentation.
        if let Some(submitted_at) = self.search.submitted_at {
            tracing::debug!(target: "little_clip_search_timing",
                request_id = result.id,
                complete = result.complete,
                succeeded,
                elapsed_ms = submitted_at.elapsed().as_secs_f64() * 1000.0,
                entries = self.directory.as_ref().map_or(0, |directory| directory.entries.len()),
                "search results accepted");
        }
        if result.complete {
            self.search.submitted_at = None;
        }
    }
    fn select_navigation_result(&mut self) {
        if !self.search.automatic {
            return;
        }
        let first = self.directory.as_ref().and_then(|directory| {
            self.visible
                .first()
                .map(|index| directory.entries[*index].path.clone())
        });
        if let Some(path) = first {
            self.select(path);
            self.reveal_selection = true;
        }
    }

    fn tick_answer_columns(&mut self) {
        let (results, pipeline) = self
            .search
            .service
            .as_ref()
            .map(|service| {
                let mut snapshot = service.snapshot.lock().unwrap();
                (
                    snapshot.answers.drain(..).collect::<Vec<_>>(),
                    snapshot.answer_pipeline.clone(),
                )
            })
            .unwrap_or_default();
        let mut changed = self.search.answers.observe_pipeline(pipeline);
        for result in results {
            changed |= self.search.answers.accept(result);
        }
        if changed {
            self.rebuild_visible();
        }
        if self.pending.is_some() || self.search.pending {
            self.search.answers.cancel_pending();
            return;
        }
        let Some(directory) = &self.directory else {
            return;
        };
        if let Some(job) = self.search.answers.next(directory)
            && let Err(error) = self
                .search_service()
                .and_then(|service| service.send(service::Command::Answer(Box::new(job))))
        {
            self.search.answers.cancel_pending();
            self.search.error = Some(error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_manager::{Navigation, filesystem::read_directory};
    use std::sync::atomic::AtomicBool;

    #[test]
    fn refresh_repeats_accepted_remote_query_without_adopting_invalid_draft() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("notes.txt"), "hello").unwrap();
        let directory = read_directory(folder.path(), &AtomicBool::new(false)).unwrap();
        let mut manager = FileManager::new(folder.path().into());
        manager.pending = None;
        manager.commit(directory, Navigation::New);
        manager.search.draft = ":*.txt notes;scope:recursive".into();
        manager.apply_search();
        let previous = manager.search.query_cancel.as_ref().unwrap().clone();
        let request_id = manager.search.request_id;
        manager.search.draft = ":invalid:".into();
        manager.search.error = Some("Invalid draft".into());
        manager.error = Some("An earlier launch failed".into());
        manager.refresh();
        assert!(manager.error.is_none());
        assert!(previous.load(std::sync::atomic::Ordering::Relaxed));
        assert!(manager.search.request_id > request_id);
        assert!(manager.search.pending);
        assert!(manager.search.submitted_at.is_some());
        assert!(
            manager.pending.is_none(),
            "must not submit an ordinary directory read"
        );
        assert_eq!(manager.search.applied.as_ref().unwrap().name, "notes");
        assert_eq!(
            manager.search.applied.as_ref().unwrap().scope,
            Scope::Recursive
        );
        assert_eq!(manager.search.draft, ":invalid:");
        assert_eq!(manager.search.error.as_deref(), Some("Invalid draft"));
        manager.close_search();
        assert!(manager.search.submitted_at.is_none());
    }

    #[test]
    fn explicit_find_reenters_command_mode_from_automatic_input_but_not_other_editors() {
        let folder = tempfile::tempdir().unwrap();
        let mut manager = FileManager::new(folder.path().into());
        let ctx = egui::Context::default();
        for (focus, allowed) in [
            (ui::input_id(), true),
            (egui::Id::new("other-editor"), false),
        ] {
            manager.search.open = true;
            manager.search.automatic = true;
            manager.search.focus = false;
            manager.search.draft = "ordinary literal".into();
            let _ = ctx.run(
                egui::RawInput {
                    modifiers: egui::Modifiers::CTRL,
                    events: vec![egui::Event::Key {
                        key: egui::Key::F,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::CTRL,
                    }],
                    ..Default::default()
                },
                |ctx| {
                    ctx.memory_mut(|memory| memory.request_focus(focus));
                    assert_eq!(manager.search_navigation_keys(ctx), allowed);
                },
            );
            assert_eq!(manager.search.automatic, !allowed);
            assert_eq!(manager.search.draft, "ordinary literal");
            assert!(
                manager.search.service.is_none(),
                "Focusing search must not execute it"
            );
        }
    }

    #[test]
    fn metadata_search_keeps_invalid_drafts_and_restores_browser_selection() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("Foo_2.jpg"), vec![0; 1500]).unwrap();
        std::fs::write(folder.path().join("old_foo.jpg"), vec![0; 1500]).unwrap();
        std::fs::write(folder.path().join("foo_10.svg"), vec![0; 1500]).unwrap();
        std::fs::create_dir(folder.path().join("folder")).unwrap();
        let directory = read_directory(folder.path(), &AtomicBool::new(false)).unwrap();
        let mut manager = FileManager::new(folder.path().into());
        manager.commit(directory, Navigation::New);
        manager.select(folder.path().join("foo_10.svg"));
        manager.location_mut().unwrap().scroll = 170.0;
        manager.search.draft = ":1KB-2KiB,image,!*.svg foo".into();
        manager.apply_search();
        assert!(manager.search.error.is_none());
        let names = || {
            manager
                .visible
                .iter()
                .map(|&i| manager.directory.as_ref().unwrap().entries[i].name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(), ["Foo_2.jpg", "old_foo.jpg"]);
        manager.search.draft = ":unknown".into();
        manager.apply_search();
        assert!(manager.search.error.is_some());
        assert_eq!(manager.visible.len(), 2);
        manager.close_search();
        assert_eq!(manager.visible.len(), 4);
        assert_eq!(
            manager.location().unwrap().selected,
            Some(folder.path().join("foo_10.svg"))
        );
        assert_eq!(manager.location().unwrap().scroll, 170.0);
    }

    #[test]
    fn changing_automatic_query_cancels_its_previous_work_and_restores_selection() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("report.txt");
        std::fs::write(&path, "text").unwrap();
        let directory = read_directory(folder.path(), &AtomicBool::new(false)).unwrap();
        let mut manager = FileManager::new(folder.path().into());
        manager.commit(directory, Navigation::New);
        manager.select(path.clone());
        manager.search.open("reprot".into());
        manager.search.automatic = true;
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        manager.search.query_cancel = Some(cancel.clone());
        manager.apply_search();
        assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
        assert_eq!(manager.search.applied.as_ref().unwrap().scope, Scope::Here);
        assert_eq!(manager.location().unwrap().selected, Some(path.clone()));
        let close_cancel = std::sync::Arc::new(AtomicBool::new(false));
        manager.search.query_cancel = Some(close_cancel.clone());
        manager.close_search();
        assert!(close_cancel.load(std::sync::atomic::Ordering::Relaxed));
        assert_eq!(manager.location().unwrap().selected, Some(path));
    }

    #[test]
    fn live_edit_retires_pending_result_before_debounced_submission() {
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let mut search = Search {
            automatic: true,
            request_id: 42,
            pending: true,
            query_cancel: Some(cancel.clone()),
            ..Search::default()
        };
        search.edited(10.0);
        assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
        assert!(!search.pending);
        assert_eq!(search.request_id, 43);
        search.edited(10.1);
        let ctx = egui::Context::default();
        for (time, expected) in [(10.3, false), (10.36, true), (10.4, false)] {
            let _ = ctx.run(
                egui::RawInput {
                    time: Some(time),
                    ..Default::default()
                },
                |ctx| {
                    assert_eq!(search.live_submission(ctx), expected);
                },
            );
        }
    }

    #[test]
    fn unknown_creation_time_and_directory_sizes_do_not_match_negations() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("foo.txt"), "hello").unwrap();
        let mut directory = read_directory(folder.path(), &AtomicBool::new(false)).unwrap();
        let entry = &mut directory.entries[0];
        entry.created_time = None;
        for command in [":created:>=2020-01-01", ":!created:>=2020-01-01"] {
            assert!(!Query::parse(command).unwrap().metadata_matches(entry));
        }
        entry.directory = true;
        assert!(!Query::parse(":!>=1KB").unwrap().metadata_matches(entry));
    }
    #[test]
    fn semantic_pages_follow_whole_window_sort_and_other_searches_stay_unpaged() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("seed.txt"), "text").unwrap();
        let mut directory = read_directory(folder.path(), &AtomicBool::new(false)).unwrap();
        let seed = directory.entries[0].clone();
        directory.entries = (0..205)
            .map(|i| {
                let mut entry = seed.clone();
                entry.name = format!("file{i}.txt");
                entry.path = folder.path().join(&entry.name);
                entry.size_bytes = i;
                entry
            })
            .collect();
        let mut manager = FileManager::new(folder.path().into());
        manager.commit(directory, Navigation::New);
        manager.search.applied = Some(Query::parse(r#":emb:"test""#).unwrap());
        manager.search.result_order = (0..205)
            .map(|i| (folder.path().join(format!("file{i}.txt")), 204 - i))
            .collect();
        manager.rebuild_visible();
        assert_eq!(manager.visible.len(), 100);
        assert_eq!(manager.search.matching_count, 205);
        assert_eq!(
            manager.directory.as_ref().unwrap().entries[manager.visible[0]].name,
            "file204.txt"
        );
        manager.search.result_pages = 1;
        manager.rebuild_visible();
        assert_eq!(manager.visible.len(), 200);
        manager.search.result_pages = 2;
        manager.rebuild_visible();
        assert_eq!(manager.visible.len(), 205);
        manager.search.result_pages = 0;
        manager.search.applied = Some(Query::parse(r#":emb:"test";sort:size:asc"#).unwrap());
        manager.rebuild_visible();
        assert_eq!(manager.visible.len(), 100);
        let directory = manager.directory.as_ref().unwrap();
        assert_eq!(directory.entries[manager.visible[0]].size_bytes, 0);
        assert_eq!(
            directory.entries[*manager.visible.last().unwrap()].size_bytes,
            99
        );
        manager.search.applied = Some(Query::parse("file").unwrap());
        manager.rebuild_visible();
        assert_eq!(manager.visible.len(), 205);
    }

    #[test]
    fn semantic_order_survives_rebuild_and_closing_cancels_the_request() {
        let folder = tempfile::tempdir().unwrap();
        for name in ["a.txt", "z.txt"] {
            std::fs::write(folder.path().join(name), "text").unwrap();
        }
        let directory = read_directory(folder.path(), &AtomicBool::new(false)).unwrap();
        let mut manager = FileManager::new(folder.path().into());
        manager.commit(directory.clone(), Navigation::New);
        manager.search.restore_directory = Some(directory);
        manager.search.applied = Some(Query::parse(":emb:\"test\"").unwrap());
        manager
            .search
            .result_order
            .insert(folder.path().join("z.txt"), 0);
        manager
            .search
            .result_order
            .insert(folder.path().join("a.txt"), 1);
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        manager.search.query_cancel = Some(cancel.clone());
        manager.rebuild_visible();
        assert_eq!(
            manager.directory.as_ref().unwrap().entries[manager.visible[0]].name,
            "z.txt"
        );
        manager.sort_by(crate::file_manager::sorting::Column::Name);
        assert!(
            !manager.sort.descending,
            "The first header click replaces relevance with ascending name order"
        );
        assert_eq!(
            manager.directory.as_ref().unwrap().entries[manager.visible[0]].name,
            "a.txt"
        );
        manager.close_search();
        assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
        assert_eq!(
            manager.directory.as_ref().unwrap().entries[manager.visible[0]].name,
            "a.txt"
        );
    }
}
