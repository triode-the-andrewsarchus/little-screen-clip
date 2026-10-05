use std::collections::BTreeSet;
use std::path::PathBuf;

use super::FileManager;

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

    fn selection_range(&self, anchor: &PathBuf, end: &PathBuf) -> Vec<PathBuf> {
        if self.visible.is_empty() {
            return Vec::new();
        }
        let Some(directory) = &self.directory else {
            return Vec::new();
        };
        let position = |path| {
            self.visible
                .iter()
                .position(|i| &directory.entries[*i].path == path)
        };
        let end_index = position(end).unwrap_or(0);
        let start_index = position(anchor).unwrap_or(end_index);
        self.visible[start_index.min(end_index)..=start_index.max(end_index)]
            .iter()
            .map(|i| directory.entries[*i].path.clone())
            .collect()
    }

    pub(super) fn select_all(&mut self) {
        let paths: BTreeSet<_> = self
            .directory
            .as_ref()
            .map(|directory| {
                self.visible
                    .iter()
                    .map(|i| directory.entries[*i].path.clone())
                    .collect()
            })
            .unwrap_or_default();
        if let Some(location) = self.location_mut() {
            if location.selected.is_none() {
                location.selected = paths.first().cloned();
            }
            location.anchor = location.selected.clone();
            location.selection = paths;
        }
        self.selection_revision = self.selection_revision.wrapping_add(1);
    }

    pub(super) fn clear_selection(&mut self) {
        if let Some(location) = self.location_mut() {
            location.selection.clear();
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
            .directory
            .as_ref()
            .map(|directory| {
                self.visible
                    .iter()
                    .map(|i| &directory.entries[*i].path)
                    .collect()
            })
            .unwrap_or_default();
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
