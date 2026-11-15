//! Visible folder/archive cards share bounded, cancellable inventory ownership.
pub(in crate::file_manager) mod archive;
pub(in crate::file_manager) mod inventory;
mod strip;
#[cfg(test)]
mod tests;

use super::{filesystem::Entry, previews::EntryPreviews};
use crate::{icons::Icon, ui_appearance::WorkbenchColors};
use egui::{Align2, FontId, Rect, Sense, Ui, pos2, vec2};
use inventory::{Child, Inventory};
use little_clip_core::{BackgroundJobs, JobError, jobs::task::Task, palette::AppPalette};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

/// The host owns ordinary row painting/actions; contained cards retain their
/// own geometry while nested section headers reuse the host's table columns.
pub(super) enum Target<'a> {
    Enter(&'a egui::Response),
    Card(&'a egui::Response),
    Member(&'a egui::Response, usize),
}

const CACHE: usize = 32;
const READS: usize = 2;
const CHOICES: usize = 4096;
pub(super) const DEPTH: usize = 3;
const FRESHNESS: Duration = Duration::from_secs(5);
// Directory metadata may stall on a remote mount. Keep its actual executing
// reads at two, even after a UI delivery deadline, without starving navigation.
static INVENTORY_JOBS: OnceLock<BackgroundJobs> = OnceLock::new();

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Options {
    pub automatic: bool,
    pub small_items: usize,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            automatic: true,
            small_items: 8,
        }
    }
}
#[derive(Default)]
pub(super) struct Previews {
    options: Options,
    members: archive::Previews,
    nodes: BTreeMap<PathBuf, Node>,
    // Bounded layout decisions describe which observed descendants fit inline.
    choices: BTreeMap<PathBuf, bool>,
    layout_changed: bool,
    protected: BTreeSet<PathBuf>,
    frame_paths: BTreeSet<PathBuf>,
    sequence: u64,
    pub organization: Arc<super::organization::Snapshot>,
    pub set_filter: super::organization::Filter,
}
struct Node {
    revision: u64,
    task: Option<Task<Result<Inventory, String>>>,
    ready: Option<Arc<Inventory>>,
    error: Option<String>,
    refreshed: Instant,
    expanded: Option<bool>,
    touched: u64,
}
impl Node {
    fn new(revision: u64) -> Self {
        Self {
            revision,
            task: None,
            ready: None,
            error: None,
            refreshed: Instant::now(),
            expanded: None,
            touched: 0,
        }
    }
    fn revise(&mut self, revision: u64) {
        if self.revision != revision {
            let expanded = self.expanded;
            *self = Self::new(revision);
            self.expanded = expanded;
        }
    }
    fn expanded(&self, options: Options) -> bool {
        self.expanded.unwrap_or_else(|| {
            options.automatic && {
                self.ready
                    .as_ref()
                    .is_none_or(|r| r.complete && r.observed <= options.small_items)
            }
        })
    }
}

pub(super) fn candidate(entry: &Entry) -> bool {
    entry.system.is_none()
        && !entry.symlink
        && (entry.directory || (entry.regular_file && inventory::archive(&entry.path)))
}
impl Previews {
    pub fn configure(&mut self, automatic: bool, small_items: usize) {
        let options = Options {
            automatic,
            small_items: small_items.clamp(1, inventory::LIMIT),
        };
        if self.options != options {
            self.options = options;
            for node in self.nodes.values_mut() {
                if !node.expanded(options) {
                    node.task = None;
                }
            }
            self.layout_changed = true;
        }
    }
    pub fn settings(&mut self, ui: &mut Ui) {
        let mut options = self.options;
        ui.menu_button("Folder previews", |ui| {
            ui.set_width(360.0);
            ui.checkbox(
                &mut options.automatic,
                "Auto-expand small folders and archives",
            );
            ui.add(
                egui::Slider::new(&mut options.small_items, 1..=inventory::LIMIT)
                    .text("Maximum items"),
            );
            ui.label("Open folders with the square folder icon.");
            ui.small("Direct contents come first. Spare space can show smaller nested contents, up to three layers. At most 32 container observations are retained.");
        });
        self.configure(options.automatic, options.small_items);
    }
    pub fn clear(&mut self) {
        self.members.clear();
        self.nodes.clear();
        self.frame_paths.clear();
        self.protected.clear();
    }
    pub fn protect(&mut self, selected: Option<&BTreeSet<PathBuf>>) {
        self.protected = selected
            .into_iter()
            .flatten()
            .flat_map(|path| path.ancestors().skip(1).take(DEPTH).map(PathBuf::from))
            .collect();
    }
    pub fn retain(&mut self, visible: &BTreeSet<PathBuf>) {
        // Metadata snapshots are the expanded location model, not thumbnail
        // payloads. Retain bounded observations for keyboard/file actions while
        // retiring every hidden read. Admission evicts unselected old snapshots.
        self.nodes.retain(|path, node| {
            let displayed = visible.contains(path);
            if !displayed {
                node.task = None;
            }
            node.ready.is_some() || displayed
        });
        self.frame_paths.clear();
    }
    pub fn finish_frame(&mut self, visible: &mut BTreeSet<PathBuf>) {
        self.members.finish();
        // Nested sections request only when their own row is visible. Merge
        // those admissions with the outer list's visible container rows.
        visible.append(&mut self.frame_paths);
        self.retain(visible);
    }
    pub fn retire(&mut self) {
        self.members.clear();
        self.retain(&BTreeSet::new());
    }
    pub fn reconcile_roots(&mut self, entries: &[Entry]) {
        let revisions: BTreeMap<_, _> = entries
            .iter()
            .filter(|entry| candidate(entry))
            .flat_map(|entry| self.sections(entry, true).0)
            .filter(|(entry, _)| self.nodes.contains_key(&entry.path))
            .map(|(entry, _)| (entry.path.clone(), entry.revision))
            .collect();
        self.nodes.retain(|path, node| {
            let Some(revision) = revisions.get(path) else {
                return false;
            };
            node.revise(*revision);
            true
        });
    }
    pub fn pending_selection(&self, path: &std::path::Path, unknown: &BTreeSet<PathBuf>) -> bool {
        for parent in path.ancestors().skip(1).take(DEPTH) {
            if unknown.contains(parent) {
                return true;
            }
            if !self.explicitly_expanded(parent) {
                break;
            }
        }
        false
    }
    pub fn unknown_children(&self, entry: &Entry) -> bool {
        self.base_height(entry) > 28.0
            && self
                .nodes
                .get(&entry.path)
                .and_then(|node| node.ready.as_ref())
                .is_none_or(|ready| !ready.complete)
    }
    fn set_visible(&self, child: &Child) -> bool {
        match child {
            Child::File(entry) => {
                entry.directory || self.organization.sets.matches(&entry.path, self.set_filter)
            }
            Child::Member { .. } => true,
        }
    }
    fn direct_children<'a>(&'a self, entry: &Entry, hidden: bool) -> Vec<&'a Entry> {
        self.nodes
            .get(&entry.path)
            .filter(|node| node.revision == entry.revision && node.expanded(self.options))
            .and_then(|node| node.ready.as_ref())
            .into_iter()
            .flat_map(|ready| &ready.children)
            .filter_map(|child| match child {
                Child::File(entry) if (hidden || !entry.hidden) && self.set_visible(child) => {
                    Some(entry.as_ref())
                }
                _ => None,
            })
            .collect()
    }
    fn base_height(&self, entry: &Entry) -> f32 {
        if self.nodes.get(&entry.path).map_or_else(
            || {
                self.choices
                    .get(&entry.path)
                    .copied()
                    .unwrap_or(self.options.automatic)
            },
            |node| node.expanded(self.options),
        ) {
            108.0
        } else {
            28.0
        }
    }
    pub fn explicitly_expanded(&self, path: &std::path::Path) -> bool {
        self.choices.get(path) == Some(&true)
    }
    fn sections<'a>(&'a self, entry: &'a Entry, hidden: bool) -> (Vec<(&'a Entry, usize)>, bool) {
        let mut rows = Vec::new();
        let mut truncated = false;
        self.collect_sections(entry, hidden, 0, &mut rows, &mut truncated);
        (rows, truncated)
    }
    fn collect_sections<'a>(
        &'a self,
        entry: &'a Entry,
        hidden: bool,
        depth: usize,
        rows: &mut Vec<(&'a Entry, usize)>,
        truncated: &mut bool,
    ) {
        if rows.len() >= CACHE {
            *truncated = true;
            return;
        }
        rows.push((entry, depth));
        if depth + 1 < DEPTH && self.base_height(entry) > 28.0 {
            for child in self.direct_children(entry, hidden) {
                if candidate(child) && self.explicitly_expanded(&child.path) {
                    self.collect_sections(child, hidden, depth + 1, rows, truncated);
                }
            }
        }
    }
    pub fn children<'a>(&'a self, entry: &'a Entry, hidden: bool) -> Vec<&'a Entry> {
        self.sections(entry, hidden)
            .0
            .into_iter()
            .flat_map(|(section, _)| self.direct_children(section, hidden))
            .collect()
    }
    pub fn height(&self, _entry: &Entry, _hidden: bool) -> f32 {
        108.0
    }
    pub fn search_scope(
        &self,
        directory: &super::filesystem::Directory,
        hidden: bool,
    ) -> Result<Option<(clip_search::here::HereScope, Vec<Entry>)>, String> {
        let mut scope = clip_search::here::HereScope::default();
        let mut entries = Vec::new();
        let mut expanded = false;
        for root in directory
            .entries
            .iter()
            .filter(|entry| entry.directory && candidate(entry) && (hidden || !entry.hidden))
        {
            let eligible = self.explicitly_expanded(&root.path)
                || self
                    .nodes
                    .get(&root.path)
                    .is_some_and(|node| node.ready.is_some() && node.expanded(self.options));
            if !eligible {
                continue;
            }
            if !expanded {
                scope.directories.push(clip_search::source::SourceRoot::from_directory(directory)
                    .ok_or("Folder source observations are unavailable; refresh before expanded search")?);
                expanded = true;
            }
            let (sections, truncated) = self.sections(root, hidden);
            scope.incomplete += usize::from(truncated);
            for (entry, _) in sections.into_iter().filter(|(entry, _)| entry.directory) {
                let source = self
                    .nodes
                    .get(&entry.path)
                    .filter(|node| node.revision == entry.revision)
                    .and_then(|node| node.ready.as_ref())
                    .and_then(|ready| ready.source.as_ref());
                if scope.observe(entry, source) {
                    // Retain known metadata for hidden-file toggles; query admission still
                    // applies the current hidden flag and source-reader policy.
                    entries.extend(self.direct_children(entry, true).into_iter().cloned());
                }
            }
        }
        Ok(expanded.then_some((scope, entries)))
    }
    pub fn selection_offset(
        &self,
        entry: &Entry,
        selected: &std::path::Path,
        _hidden: bool,
    ) -> f32 {
        if selected == entry.path { 0.0 } else { 30.0 }
    }
    fn choose(&mut self, path: PathBuf, expanded: bool) {
        if self.choices.get(&path) == Some(&expanded) {
            return;
        }
        if self.choices.len() >= CHOICES && !self.choices.contains_key(&path) {
            return;
        }
        self.choices.insert(path.clone(), expanded);
        if let Some(node) = self.nodes.get_mut(&path) {
            node.expanded = Some(expanded);
            if !expanded {
                node.task = None;
            }
        }
        self.layout_changed = true;
    }
    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        self.members.begin(ctx);
        let mut changed = std::mem::take(&mut self.layout_changed);
        for node in self.nodes.values_mut() {
            if let Some(result) = node.task.as_ref().and_then(Task::poll) {
                node.task = None;
                match result {
                    Ok(Ok(ready)) => {
                        node.ready = Some(Arc::new(ready));
                        node.error = None;
                    }
                    Err(JobError::QueueFull) => {
                        node.refreshed = Instant::now() - FRESHNESS;
                    }
                    Ok(Err(error)) => {
                        node.ready = None;
                        node.error = Some(error);
                    }
                    Err(error) => {
                        node.ready = None;
                        node.error = Some(error.to_string());
                    }
                }
                changed = true;
            }
            if node.task.is_some() {
                ctx.request_repaint_after(Duration::from_millis(40));
            }
        }
        changed
    }
    pub(super) fn request(&mut self, entry: &Entry) {
        self.request_for_layout(entry, false);
    }
    fn request_for_layout(&mut self, entry: &Entry, nested: bool) {
        self.frame_paths.insert(entry.path.clone());
        if !self.nodes.contains_key(&entry.path) && self.nodes.len() >= CACHE {
            let oldest = self
                .nodes
                .iter()
                .filter(|(path, _)| {
                    !self.protected.contains(*path) && !self.frame_paths.contains(*path)
                })
                .min_by_key(|(_, node)| node.touched)
                .map(|(path, _)| path.clone());
            let Some(oldest) = oldest else {
                return;
            };
            self.nodes.remove(&oldest);
        }
        self.sequence = self.sequence.wrapping_add(1);
        let active = self.nodes.values().filter(|n| n.task.is_some()).count();
        let choice = self.choices.get(&entry.path).copied();
        let node = self.nodes.entry(entry.path.clone()).or_insert_with(|| {
            let mut node = Node::new(entry.revision);
            node.expanded = choice;
            node
        });
        node.revise(entry.revision);
        node.touched = self.sequence;
        if (!nested && !node.expanded(self.options)) || active >= READS || node.task.is_some() {
            return;
        }
        if (node.ready.is_some() || node.error.is_some()) && node.refreshed.elapsed() < FRESHNESS {
            return;
        }
        let entry = entry.clone();
        node.task = Some(
            Task::start(
                INVENTORY_JOBS.get_or_init(|| BackgroundJobs::new(READS, READS)),
                Duration::from_secs(10),
                move |cancel| inventory::load(&entry, cancel),
            )
            .with_response_deadline(Duration::from_secs(10)),
        );
        node.refreshed = Instant::now();
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "Shared preview and host interaction boundary"
    )]
    pub fn show(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        entry: &Entry,
        files: &mut EntryPreviews,
        keep: &mut BTreeSet<PathBuf>,
        hidden: bool,
        colors: WorkbenchColors,
        _palette: &AppPalette,
        selected: Option<&BTreeSet<PathBuf>>,
        can_request: bool,
        mut interact: impl FnMut(&mut Ui, &Entry, Target<'_>),
    ) {
        self.show_strip(
            ui,
            rect,
            entry,
            files,
            keep,
            hidden,
            colors,
            selected,
            can_request,
            &mut interact,
        );
    }
}
