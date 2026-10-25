use std::collections::BTreeSet;
use std::path::PathBuf;

use super::FileManager;

/// One prior multi-selection per folder, including its keyboard/range anchors.
#[derive(Clone, Debug)]
pub(super) struct SelectionSnapshot {
    pub(super) paths: BTreeSet<PathBuf>,
    pub(super) selected: Option<PathBuf>,
    pub(super) anchor: Option<PathBuf>,
}

impl FileManager {
    pub(super) fn selection_count(&self) -> usize {
        self.location()
            .map_or(0, |location| location.selection.len())
    }

    pub(super) fn select(&mut self, path: PathBuf) {
        self.select_with(path, false, false);
    }

    pub(super) fn select_with(&mut self, path: PathBuf, additive: bool, range: bool) {
        let anchor = self.location().and_then(|location| location.anchor.clone());
        let paths = if range {
            self.selection_range(anchor.as_ref().unwrap_or(&path), &path)
        } else {
            vec![path.clone()]
        };
        if let Some(location) = self.location_mut() {
            if additive || range {
                location.selection_undo = None;
            } else if location.selection.len() > 1 {
                location.selection_undo = Some(SelectionSnapshot {
                    paths: std::mem::take(&mut location.selection),
                    selected: location.selected.clone(),
                    anchor: location.anchor.clone(),
                });
            }
            if !additive {
                location.selection.clear();
            }
            if additive && !range && !location.selection.insert(path.clone()) {
                location.selection.remove(&path);
            } else {
                location.selection.extend(paths);
            }
            if !range || location.anchor.is_none() {
                location.anchor = Some(path.clone());
            }
            location.selected = Some(path);
        }
        self.selection_revision = self.selection_revision.wrapping_add(1);
    }

    pub(super) fn selection_shortcuts(&mut self, ctx: &egui::Context) {
        let text_focused = ctx
            .memory(|memory| memory.focused())
            .is_some_and(|id| egui::TextEdit::load_state(ctx, id).is_some());
        if !text_focused && super::ui::key(ctx, egui::Modifiers::CTRL, egui::Key::Z) {
            self.undo_selection();
            ctx.request_repaint();
        }
    }

    pub(super) fn undo_selection(&mut self) {
        let Some(snapshot) = self
            .location_mut()
            .and_then(|location| location.selection_undo.take())
        else {
            return;
        };
        if let Some(location) = self.location_mut() {
            location.selection = snapshot.paths;
            location.selected = snapshot.selected;
            location.anchor = snapshot.anchor;
        }
        // Refreshes and hidden-file changes must not resurrect unavailable rows.
        self.reconcile_selection();
        self.selection_revision = self.selection_revision.wrapping_add(1);
        self.reveal_selection = true;
    }

    /// Entries owned by the active view; recursive Photos is independent of root rows.
    pub(super) fn displayed_entries(&self) -> Vec<&super::filesystem::Entry> {
        if self.view == super::BrowserView::Photos
            && self.photo_mode == super::gallery::similarity::PhotoMode::Similarity
            && self.similarity_canvas.is_current_directory(self.path())
        {
            return self.similarity_canvas.entries.iter().collect();
        }
        self.directory
            .iter()
            .flat_map(|directory| {
                self.visible
                    .iter()
                    .filter_map(|&index| directory.entries.get(index))
            })
            .filter(|entry| self.view != super::BrowserView::Photos || entry.is_photo())
            .collect()
    }

    fn selection_range(&self, anchor: &PathBuf, end: &PathBuf) -> Vec<PathBuf> {
        let entries = self.displayed_entries();
        let position = |path| entries.iter().position(|entry| &entry.path == path);
        let Some(end_index) = position(end) else {
            return Vec::new();
        };
        let start_index = position(anchor).unwrap_or(end_index);
        entries[start_index.min(end_index)..=start_index.max(end_index)]
            .iter()
            .map(|entry| entry.path.clone())
            .collect()
    }

    pub(super) fn select_all(&mut self) {
        let paths: BTreeSet<_> = self
            .displayed_entries()
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        if let Some(location) = self.location_mut() {
            if location
                .selected
                .as_ref()
                .is_none_or(|selected| !paths.contains(selected))
            {
                location.selected = paths.first().cloned();
            }
            location.anchor = location.selected.clone();
            location.selection = paths;
            location.selection_undo = None;
        }
        self.selection_revision = self.selection_revision.wrapping_add(1);
    }

    pub(super) fn select_all_paths(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        let path_set: BTreeSet<PathBuf> = paths.into_iter().collect();
        if let Some(location) = self.location_mut() {
            location.selected = path_set.iter().next().cloned();
            location.anchor = location.selected.clone();
            location.selection = path_set;
            location.selection_undo = None;
        }
        self.selection_revision = self.selection_revision.wrapping_add(1);
    }

    pub(super) fn clear_selection(&mut self) {
        if let Some(location) = self.location_mut() {
            location.selection.clear();
            location.selection_undo = None;
            location.selected = None;
            location.anchor = None;
        }
        self.selection_revision = self.selection_revision.wrapping_add(1);
    }

    pub(super) fn reconcile_selection(&mut self) {
        let Some(index) = self.index else {
            return;
        };
        let paths: BTreeSet<_> = self
            .displayed_entries()
            .iter()
            .map(|entry| entry.path.clone())
            .collect();
        let location = &mut self.history[index];
        location.selection.retain(|path| paths.contains(path));
        if location
            .selected
            .as_ref()
            .is_some_and(|path| !paths.contains(path))
        {
            location.selected = None;
        }
        if location
            .anchor
            .as_ref()
            .is_some_and(|path| !paths.contains(path))
        {
            location.anchor = location.selected.clone();
        }
    }
}
