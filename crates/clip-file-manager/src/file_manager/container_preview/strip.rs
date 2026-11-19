//! Direct contents have first claim on space; nested contents never add rows.
use super::*;
use std::collections::VecDeque;

const GAP: f32 = 6.0;
pub(super) fn edge(level: usize) -> f32 {
    [64.0, 44.0, 32.0][level.min(DEPTH - 1)]
}
fn visible(child: &Child, hidden: bool) -> bool {
    hidden || !matches!(child, Child::File(entry) if entry.hidden)
}

pub(super) struct Tile {
    pub(super) child: Child,
    container: Entry,
    source: Option<little_clip_core::file_source::FileSource>,
    pub(super) level: usize,
}
impl Previews {
    pub(super) fn plan_strip(
        &mut self,
        entry: &Entry,
        width: f32,
        hidden: bool,
        can_request: bool,
    ) {
        let mut expanded = BTreeSet::new();
        let root = self
            .nodes
            .get(&entry.path)
            .filter(|node| node.expanded(self.options))
            .and_then(|node| node.ready.clone());
        if let Some(root) = root {
            let direct = root
                .children
                .iter()
                .filter(|child| visible(child, hidden))
                .count();
            let mut spare = (width - direct as f32 * (edge(0) + GAP)).max(0.0);
            let mut queue = VecDeque::new();
            if root.complete {
                enqueue(&root, hidden, 1, &mut queue);
            }
            while let Some((child, level)) = queue.pop_front() {
                if level >= DEPTH || spare < edge(level) + GAP {
                    continue;
                }
                if can_request {
                    self.request_for_layout(&child, true);
                }
                let ready = self
                    .nodes
                    .get(&child.path)
                    .filter(|node| node.revision == child.revision)
                    .and_then(|node| node.ready.clone());
                let Some(ready) = ready.filter(|ready| ready.complete) else {
                    continue;
                };
                let count = ready
                    .children
                    .iter()
                    .filter(|child| visible(child, hidden))
                    .count();
                let needed = count as f32 * (edge(level) + GAP);
                if count > 0 && needed <= spare {
                    spare -= needed;
                    expanded.insert(child.path.clone());
                    enqueue(&ready, hidden, level + 1, &mut queue);
                }
            }
        }
        let previous: Vec<_> = self
            .choices
            .keys()
            .filter(|path| path.as_path() != entry.path && path.starts_with(&entry.path))
            .cloned()
            .collect();
        for path in previous {
            self.choose(path.clone(), expanded.contains(&path));
        }
        for path in expanded {
            self.choose(path, true);
        }
    }
    pub(super) fn tiles(
        &self,
        container: &Entry,
        hidden: bool,
        level: usize,
        output: &mut Vec<Tile>,
    ) {
        if level >= DEPTH {
            return;
        }
        let Some(ready) = self
            .nodes
            .get(&container.path)
            .filter(|node| node.revision == container.revision && node.expanded(self.options))
            .and_then(|node| node.ready.as_ref())
        else {
            return;
        };
        for child in ready.children.iter().filter(|child| visible(child, hidden)) {
            output.push(Tile {
                child: child.clone(),
                container: container.clone(),
                source: ready.archive_source.clone(),
                level,
            });
            if let Child::File(entry) = child
                && self.explicitly_expanded(&entry.path)
            {
                self.tiles(entry, hidden, level + 1, output);
            }
        }
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "Shared strip rendering/interaction boundary"
    )]
    pub(super) fn show_strip(
        &mut self,
        ui: &mut Ui,
        rect: Rect,
        entry: &Entry,
        files: &mut EntryPreviews,
        keep: &mut BTreeSet<PathBuf>,
        hidden: bool,
        colors: WorkbenchColors,
        selected: Option<&BTreeSet<PathBuf>>,
        can_request: bool,
        interact: &mut dyn FnMut(&mut Ui, &Entry, Target<'_>),
    ) {
        let enter = Rect::from_min_size(rect.min + vec2(4.0, 34.0), vec2(edge(0), edge(0)));
        let response = ui
            .interact(
                enter,
                ui.scope_id().with(("enter-container", &entry.path)),
                Sense::click_and_drag(),
            )
            .on_hover_text("Open folder");
        ui.painter().rect_filled(
            enter,
            4.0,
            if response.hovered() {
                colors.panel
            } else {
                colors.recessed
            },
        );
        super::super::ui::entry_icon(entry).paint(
            ui.painter(),
            Rect::from_center_size(enter.center() - vec2(0.0, 6.0), vec2(40.0, 40.0)),
            colors.text_primary,
        );
        ui.painter().text(
            enter.center_bottom() - vec2(0.0, 3.0),
            Align2::CENTER_BOTTOM,
            "Open",
            FontId::proportional(11.0),
            colors.text_secondary,
        );
        interact(ui, entry, Target::Enter(&response));
        let body = Rect::from_min_max(rect.min + vec2(78.0, 30.0), rect.max - vec2(4.0, 4.0));
        self.plan_strip(entry, body.width().max(0.0), hidden, can_request);
        let Some(node) = self.nodes.get(&entry.path) else {
            return;
        };
        let ready = node.ready.clone();
        if !node.expanded(self.options) {
            ui.painter().text(
                body.left_center(),
                Align2::LEFT_CENTER,
                "Open to browse contents",
                FontId::proportional(12.0),
                colors.text_secondary,
            );
            return;
        }
        ui.painter().rect_filled(body, 4.0, colors.recessed);
        let Some(ready) = ready else {
            ui.painter().text(
                body.left_center() + vec2(6.0, 0.0),
                Align2::LEFT_CENTER,
                node.error.as_deref().unwrap_or("Reading contained files…"),
                FontId::proportional(12.0),
                colors.text_secondary,
            );
            return;
        };
        ui.painter().text(
            body.min + vec2(4.0, 0.0),
            Align2::LEFT_TOP,
            format!(
                "{} · {}{} items",
                if ready.archive {
                    "Archive contents"
                } else {
                    "Inside folder"
                },
                ready.observed,
                if ready.complete { "" } else { "+" }
            ),
            FontId::proportional(10.0),
            colors.text_secondary,
        );
        let mut tiles = Vec::new();
        self.tiles(entry, hidden, 0, &mut tiles);
        ui.scope_builder(egui::UiBuilder::new().max_rect(Rect::from_min_max(body.min + vec2(0.0, 10.0), body.max)), |ui| {
            egui::ScrollArea::horizontal().id_salt(("container-cards", &entry.path)).auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                ui.horizontal_centered(|ui| {
                    for tile in &tiles {
                        let marked = matches!(&tile.child, Child::File(entry) if selected.is_some_and(|paths| paths.contains(&entry.path)));
                        let response = card(ui, tile, files, &mut self.members, keep, colors, marked);
                        match &tile.child {
                            Child::File(entry) if entry.directory => interact(ui, entry, Target::Enter(&response)),
                            Child::File(entry) => interact(ui, entry, Target::Card(&response)),
                            Child::Member { ordinal, .. } => interact(ui, &tile.container, Target::Member(&response, *ordinal)),
                        }
                    }
                    if tiles.is_empty() { ui.label(if ready.complete { "Empty" } else { "No previewable entries" }); }
                    if ready.omitted > 0 { ui.label(format!("{} omitted", ready.omitted)); }
                    if !ready.complete { ui.label("More inside").on_hover_text(ready.notice.as_deref().unwrap_or("Open the folder to see more contents")); }
                });
            });
        });
    }
}
fn enqueue(
    inventory: &Inventory,
    hidden: bool,
    level: usize,
    queue: &mut VecDeque<(Entry, usize)>,
) {
    for child in inventory
        .children
        .iter()
        .filter(|child| visible(child, hidden))
    {
        if let Child::File(entry) = child
            && candidate(entry)
        {
            queue.push_back((entry.as_ref().clone(), level));
        }
    }
}
fn card(
    ui: &mut Ui,
    tile: &Tile,
    files: &mut EntryPreviews,
    members: &mut archive::Previews,
    keep: &mut BTreeSet<PathBuf>,
    colors: WorkbenchColors,
    marked: bool,
) -> egui::Response {
    let size = edge(tile.level);
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::click_and_drag());
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
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
    if tile.level > 0 {
        painter.rect_stroke(
            rect,
            3.0,
            egui::Stroke::new(1.0_f32, colors.accent.gamma_multiply(0.25)),
            egui::StrokeKind::Middle,
        );
    }
    let ready = if ui.clip_rect().intersects(rect) {
        match &tile.child {
            Child::File(entry) if !entry.directory && !entry.symlink => {
                keep.insert(entry.path.clone());
                files.listing(entry)
            }
            Child::Member {
                ordinal,
                directory: false,
                ..
            } => tile
                .source
                .as_ref()
                .and_then(|source| members.request(source, *ordinal, Default::default())),
            _ => None,
        }
    } else {
        None
    };
    let icon_size = size * 0.62;
    let icon_rect = Rect::from_center_size(
        pos2(rect.center().x, rect.top() + icon_size / 2.0 + 2.0),
        vec2(icon_size, icon_size),
    );
    if let Some(texture) = ready.and_then(|ready| ready.texture.as_ref()) {
        let target = Rect::from_center_size(
            icon_rect.center(),
            texture.size_vec2() * (icon_size / texture.size_vec2().max_elem()),
        );
        painter.image(
            texture.id(),
            target,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        let icon = match &tile.child {
            Child::File(entry) => super::super::ui::entry_icon(entry),
            _ if tile.child.directory() => Icon::Folder,
            _ => Icon::Text,
        };
        icon.paint(&painter, icon_rect, colors.text_secondary);
    }
    let label = tile
        .child
        .name()
        .rsplit('/')
        .next()
        .unwrap_or(tile.child.name());
    let font = FontId::proportional([11.0, 9.0, 8.0][tile.level.min(DEPTH - 1)]);
    let galley = painter.layout_no_wrap(label.into(), font, colors.text_primary);
    painter.galley(
        pos2(rect.left() + 2.0, rect.bottom() - galley.size().y - 2.0),
        galley,
        colors.text_primary,
    );
    response.on_hover_ui(|ui| {
        ui.label(match &tile.child {
            Child::File(entry) => super::super::filesystem::display_path(&entry.path),
            Child::Member { name, .. } => format!("Archive member: {name}"),
        });
        if let Some(ready) = ready {
            ui.label(&ready.data.detail);
            if let Some(texture) = &ready.texture {
                ui.image((texture.id(), texture.size_vec2()));
            }
            if !ready.data.text.is_empty() {
                ui.monospace(ready.data.text.chars().take(240).collect::<String>());
            }
        }
    })
}
