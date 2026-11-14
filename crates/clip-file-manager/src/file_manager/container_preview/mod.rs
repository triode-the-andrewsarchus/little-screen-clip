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

const CACHE: usize = 32;
const READS: usize = 2;
const CHOICES: usize = 4096;
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
            .filter_map(|path| path.parent().map(PathBuf::from))
            .collect();
    }
    pub fn retain(&mut self, visible: &BTreeSet<PathBuf>) {
        // Metadata snapshots are the expanded location model, not thumbnail
        // payloads. Retain bounded observations for keyboard/file actions while
        // retiring every hidden read. Admission evicts unselected old snapshots.
        self.nodes.retain(|path, node| {
            if !visible.contains(path) {
                node.task = None;
            }
            node.ready.is_some() || visible.contains(path)
        });
        self.frame_paths.clear();
    }
    pub fn retire(&mut self) {
        self.retain(&BTreeSet::new());
    }
    pub fn reconcile_roots(&mut self, entries: &[Entry]) {
        self.nodes.retain(|path, node| {
            let Some(entry) = entries.iter().find(|entry| &entry.path == path) else {
                return false;
            };
            if node.revision != entry.revision {
                let choice = node.expanded;
                *node = Node::new(entry.revision);
                node.expanded = choice;
            }
            true
        });
    }
    pub fn unknown_children(&self, entry: &Entry) -> bool {
        self.height(entry) > 28.0
            && self
                .nodes
                .get(&entry.path)
                .and_then(|node| node.ready.as_ref())
                .is_none_or(|ready| !ready.complete)
    }
    pub fn children<'a>(&'a self, entry: &Entry, hidden: bool) -> Vec<&'a Entry> {
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
    pub fn height(&self, entry: &Entry) -> f32 {
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
        if node.revision != entry.revision {
            let expanded = node.expanded;
            *node = Node::new(entry.revision);
            node.expanded = expanded;
        }
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
        mut interact: impl FnMut(&mut Ui, &egui::Response, &Entry),
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
                node.expanded = Some(!expanded);
                self.choices.insert(entry.path.clone(), !expanded);
                if expanded {
                    node.task = None;
                }
                self.layout_changed = true;
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
        let Some(ready) = &node.ready else {
            ui.painter().text(
                body.left_center() + vec2(10.0, 0.0),
                Align2::LEFT_CENTER,
                node.error.as_deref().unwrap_or("Reading contained files…"),
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
        ui.allocate_ui_at_rect(cards, |ui| {
            egui::ScrollArea::horizontal()
                .id_source(("container-cards", &entry.path))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    inventory_cards(
                        ui,
                        ready,
                        files,
                        keep,
                        hidden,
                        colors,
                        selected,
                        &mut interact,
                    );
                });
        });
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
    interact: &mut impl FnMut(&mut Ui, &egui::Response, &Entry),
) {
    ui.horizontal(|ui| {
        for child in &ready.children {
            if !hidden && matches!(child,Child::File(e) if e.hidden) {
                continue;
            }
            let marked = matches!(child, Child::File(entry) if selected.is_some_and(|paths| paths.contains(&entry.path)));
            let response = card(ui, child, files, keep, colors, marked);
            if let Child::File(entry) = child {
                interact(ui, &response, entry);
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
            ui.label("More contents · open to browse");
        }
    });
}

fn card(
    ui: &mut Ui,
    child: &Child,
    files: &mut EntryPreviews,
    keep: &mut BTreeSet<PathBuf>,
    colors: WorkbenchColors,
    marked: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(154.0, 48.0), Sense::click_and_drag());
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
        108.0,
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
    response.on_hover_text(hover)
}
