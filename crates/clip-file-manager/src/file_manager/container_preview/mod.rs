//! Visible folder/archive cards share bounded, cancellable inventory ownership.
mod inventory;
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
    Header(Rect),
    Card(&'a egui::Response),
}

const CACHE: usize = 32;
const READS: usize = 2;
const CHOICES: usize = 4096;
pub(super) const DEPTH: usize = clip_search::here::MAX_DEPTH;
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
    nodes: BTreeMap<PathBuf, Node>,
    // User intent outlives visible observations, navigation and studio entry.
    // This is session state, keyed by path, not a payload or metadata cache.
    choices: BTreeMap<PathBuf, bool>,
    layout_changed: bool,
    protected: BTreeSet<PathBuf>,
    frame_paths: BTreeSet<PathBuf>,
    sequence: u64,
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
            ui.label("Use + or − beside a folder or archive to override.");
            ui.small("Nested previews expand explicitly, up to four levels. At most 32 container observations are retained.");
        });
        self.configure(options.automatic, options.small_items);
    }
    pub fn clear(&mut self) {
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
        // Nested sections request only when their own row is visible. Merge
        // those admissions with the outer list's visible container rows.
        visible.append(&mut self.frame_paths);
        self.retain(visible);
    }
    pub fn retire(&mut self) {
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
    fn direct_children<'a>(&'a self, entry: &Entry, hidden: bool) -> Vec<&'a Entry> {
        self.nodes
            .get(&entry.path)
            .filter(|node| node.revision == entry.revision && node.expanded(self.options))
            .and_then(|node| node.ready.as_ref())
            .into_iter()
            .flat_map(|ready| &ready.children)
            .filter_map(|child| match child {
                Child::File(entry) if hidden || !entry.hidden => Some(entry.as_ref()),
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
    pub fn height(&self, entry: &Entry, hidden: bool) -> f32 {
        let (sections, truncated) = self.sections(entry, hidden);
        sections
            .into_iter()
            .map(|(entry, _)| self.base_height(entry))
            .sum::<f32>()
            + if truncated { 24.0 } else { 0.0 }
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
    pub fn selection_offset(&self, entry: &Entry, selected: &std::path::Path, hidden: bool) -> f32 {
        let mut offset = 0.0;
        for (section, _) in self.sections(entry, hidden).0 {
            if section.path == selected {
                return offset;
            }
            if self
                .direct_children(section, hidden)
                .iter()
                .any(|child| child.path == selected)
            {
                return offset + 30.0;
            }
            offset += self.base_height(section);
        }
        0.0
    }
    fn choose(&mut self, path: PathBuf, expanded: bool) {
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
        if !node.expanded(self.options) || active >= READS || node.task.is_some() {
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
        reason = "Explicit shared preview and containment painting boundary"
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
        // Only the bounded section descriptors are copied; sample/image payloads
        // remain with their existing shared preview owners.
        let (sections, truncated) = self.sections(entry, hidden);
        let sections: Vec<_> = sections
            .into_iter()
            .map(|(entry, depth)| (entry.clone(), depth))
            .collect();
        let mut top = rect.top();
        for (section, depth) in sections {
            let height = self.base_height(&section);
            let target = Rect::from_min_max(
                pos2(rect.left() + depth as f32 * 18.0, top),
                pos2(rect.right(), top + height),
            );
            if ui.clip_rect().intersects(target) {
                if depth > 0 && can_request {
                    self.request(&section);
                }
                self.show_section(
                    ui,
                    target,
                    &section,
                    depth,
                    files,
                    keep,
                    hidden,
                    colors,
                    selected,
                    &mut interact,
                );
            }
            top += height;
        }
        if truncated {
            ui.painter().text(
                pos2(rect.left() + 18.0, top + 2.0),
                Align2::LEFT_TOP,
                "Inline preview limit reached · open folder to see more",
                FontId::proportional(12.0),
                colors.text_secondary,
            );
        }
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "Explicit section painting and host interaction boundary"
    )]
    fn show_section(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        entry: &Entry,
        depth: usize,
        files: &mut EntryPreviews,
        keep: &mut BTreeSet<PathBuf>,
        hidden: bool,
        colors: WorkbenchColors,
        selected: Option<&BTreeSet<PathBuf>>,
        interact: &mut dyn FnMut(&mut Ui, &Entry, Target<'_>),
    ) {
        let can_choose = self.choices.len() < CHOICES || self.choices.contains_key(&entry.path);
        let Some(node) = self.nodes.get_mut(&entry.path) else {
            ui.painter().text(
                rect.left_top() + vec2(18.0, 42.0),
                Align2::LEFT_TOP,
                "Preview limit reached · open folder to browse",
                FontId::proportional(12.0),
                colors.text_secondary,
            );
            return;
        };
        let expanded = node.expanded(self.options);
        let ready = node.ready.clone();
        let error = node.error.clone();
        if depth > 0 {
            let header = Rect::from_min_max(
                rect.min + vec2(26.0, 0.0),
                pos2(rect.right(), rect.top() + 28.0),
            );
            interact(ui, entry, Target::Header(header));
        }
        let toggle =
            Rect::from_min_size(pos2(rect.left() + 2.0, rect.top() + 2.0), vec2(24.0, 24.0));
        ui.allocate_ui_at_rect(toggle, |ui| {
            if ui
                .add_enabled(
                    can_choose,
                    egui::Button::new(if expanded { "−" } else { "+" }).small(),
                )
                .on_hover_text("Show or hide contained files")
                .on_disabled_hover_text("This session has reached its 4,096 folder-preview choices")
                .clicked()
            {
                self.choose(entry.path.clone(), !expanded);
                ui.ctx().request_repaint();
            }
        });
        if rect.height() <= 30.0 {
            return;
        }
        let body = Rect::from_min_max(
            pos2(rect.left() + 18.0, rect.top() + 30.0),
            rect.right_bottom() - vec2(6.0, 4.0),
        );
        ui.painter().rect_filled(body, 4.0, colors.recessed);
        ui.painter().line_segment(
            [body.left_top(), body.left_bottom()],
            egui::Stroke::new(2.0_f32, colors.text_secondary.gamma_multiply(0.4)),
        );
        let Some(ready) = ready else {
            ui.painter().text(
                body.left_center() + vec2(10.0, 0.0),
                Align2::LEFT_CENTER,
                error.as_deref().unwrap_or("Reading contained files…"),
                FontId::proportional(12.0),
                colors.text_secondary,
            );
            return;
        };
        let label = format!(
            "{} · {}{} items",
            if ready.archive {
                "Archive contents"
            } else {
                "Inside folder"
            },
            ready.observed,
            if ready.complete { "" } else { "+" }
        );
        ui.painter().text(
            body.left_top() + vec2(8.0, 2.0),
            Align2::LEFT_TOP,
            label,
            FontId::proportional(10.0),
            colors.text_secondary,
        );
        let cards = Rect::from_min_max(
            body.left_top() + vec2(6.0, 17.0),
            body.right_bottom() - vec2(4.0, 2.0),
        );
        let mut choice = None;
        ui.allocate_ui_at_rect(cards, |ui| {
            egui::ScrollArea::horizontal()
                .id_source(("container-cards", &entry.path))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    inventory_cards(
                        ui,
                        &ready,
                        files,
                        keep,
                        hidden,
                        colors,
                        selected,
                        interact,
                        &self.choices,
                        depth + 1 < DEPTH,
                        &mut choice,
                    );
                });
        });
        if let Some((path, expanded)) = choice {
            self.choose(path, expanded);
            ui.ctx().request_repaint();
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Explicit contained-card rendering and host interaction boundary"
)]
fn inventory_cards(
    ui: &mut Ui,
    ready: &Inventory,
    files: &mut EntryPreviews,
    keep: &mut BTreeSet<PathBuf>,
    hidden: bool,
    colors: WorkbenchColors,
    selected: Option<&BTreeSet<PathBuf>>,
    interact: &mut dyn FnMut(&mut Ui, &Entry, Target<'_>),
    choices: &BTreeMap<PathBuf, bool>,
    can_descend: bool,
    choice: &mut Option<(PathBuf, bool)>,
) {
    ui.horizontal(|ui| {
        for child in &ready.children {
            if !hidden && matches!(child,Child::File(e) if e.hidden) {
                continue;
            }
            let marked = matches!(child, Child::File(entry) if selected.is_some_and(|paths| paths.contains(&entry.path)));
            let (response, disclosure) = card(ui, child, files, keep, colors, marked);
            if let Child::File(entry) = child {
                interact(ui, entry, Target::Card(&response));
                if let Some(disclosure) = disclosure
                    && let Some(expanded) = nested_disclosure(ui, disclosure, entry, choices, can_descend)
                {
                    *choice = Some((entry.path.clone(), expanded));
                }
            }
        }
        if ready.children.is_empty() {
            ui.label(if ready.complete {
                "Empty"
            } else {
                "No previewable entries"
            });
        }
        if ready.omitted > 0 {
            ui.label(format!("{} entries omitted", ready.omitted))
                .on_hover_text(
                    "Links, special members or unavailable directory entries are not previewed.",
                );
        }
        if !ready.complete {
            ui.label(if ready.notice.is_some() { "Preview limit · open to browse" } else { "More contents · open to browse" })
                .on_hover_text(ready.notice.as_deref().unwrap_or("The inventory is bounded; open the container to inspect more."));
        }
    });
}

fn nested_disclosure(
    ui: &mut Ui,
    rect: Rect,
    entry: &Entry,
    choices: &BTreeMap<PathBuf, bool>,
    can_descend: bool,
) -> Option<bool> {
    let expanded = choices.get(&entry.path) == Some(&true);
    let available = can_descend && (choices.len() < CHOICES || choices.contains_key(&entry.path));
    ui.allocate_ui_at_rect(rect, |ui| {
        ui.add_enabled(
            available,
            egui::Button::new(if expanded { "−" } else { "+" }).small(),
        )
        .on_hover_text("Expand or collapse contents here; double-click the name to open")
        .on_disabled_hover_text(
            "Inline previews have a four-level and 4,096-choice limit; open the folder to continue",
        )
        .clicked()
        .then_some(!expanded)
    })
    .inner
}

fn card(
    ui: &mut Ui,
    child: &Child,
    files: &mut EntryPreviews,
    keep: &mut BTreeSet<PathBuf>,
    colors: WorkbenchColors,
    marked: bool,
) -> (egui::Response, Option<Rect>) {
    let (rect, allocated) = ui.allocate_exact_size(vec2(154.0, 48.0), Sense::hover());
    let disclosure = matches!(child, Child::File(entry) if candidate(entry)).then(|| {
        Rect::from_min_size(
            pos2(rect.right() - 25.0, rect.top() + 13.0),
            vec2(23.0, 23.0),
        )
    });
    let mut target = rect;
    if disclosure.is_some() {
        target.max.x -= 26.0;
    }
    let response = ui.interact(
        target,
        allocated.id.with("card-body"),
        Sense::click_and_drag(),
    );
    let painter = ui.painter().with_clip_rect(ui.clip_rect().intersect(rect));
    painter.rect_filled(
        rect,
        3.0,
        if marked {
            colors.accent.gamma_multiply(0.2)
        } else if response.hovered() {
            colors.panel
        } else {
            colors.canvas
        },
    );
    let ready = match child {
        Child::File(entry) if !entry.directory && !entry.symlink => {
            keep.insert(entry.path.clone());
            files.listing(entry)
        }
        _ => None,
    };
    if let Some(texture) = ready.and_then(|ready| ready.texture.as_ref()) {
        let target = Rect::from_center_size(
            rect.left_center() + vec2(22.0, 0.0),
            texture.size_vec2() * (36.0 / texture.size_vec2().max_elem()).min(1.0),
        );
        painter.image(
            texture.id(),
            target,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        let icon = ready
            .map(|ready| ready.data.icon)
            .unwrap_or(if child.directory() {
                Icon::Folder
            } else {
                Icon::Text
            });
        icon.paint(
            &painter,
            Rect::from_min_size(rect.left_top() + vec2(8.0, 14.0), vec2(20.0, 20.0)),
            colors.text_secondary,
        );
    }
    let label = painter.layout(
        child
            .name()
            .rsplit('/')
            .next()
            .unwrap_or(child.name())
            .to_owned(),
        FontId::proportional(13.0),
        colors.text_primary,
        if disclosure.is_some() { 80.0 } else { 108.0 },
    );
    painter.galley(
        rect.left_top() + vec2(43.0, 5.0),
        label,
        colors.text_primary,
    );
    let hover = match child {
        Child::File(e) => super::filesystem::display_path(&e.path),
        Child::Member { name, size, .. } => format!(
            "Archive member: {name}\n{} · read-only inventory",
            super::filesystem::format_size(*size)
        ),
    };
    (response.on_hover_text(hover), disclosure)
}
