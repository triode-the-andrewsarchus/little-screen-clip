//! Saved GNU Radio topology and inspected block settings.

mod canvas;
pub(super) mod strip;

use crate::{file_source::ReadCheck, ui_appearance::WorkbenchColors};
use canvas::{Camera, Highlights, Scene};
use egui::{Layout as UiLayout, PointerButton, ScrollArea, Sense, Ui, Vec2, vec2};
use little_clip_core::{
    engineering::flowgraph::{self, Direction, Graph, Layout, Preview, Setting, Step},
    palette::AppPalette,
};
use std::{path::Path, sync::atomic::AtomicBool};

const TRACE_LIMIT: usize = 512;

pub(super) struct Viewer {
    graph: Graph,
    layout: Layout,
    camera: Camera,
    selected: Option<usize>,
    reveal: Option<usize>,
    trace_direction: Option<Direction>,
    trace_steps: Vec<Step>,
    trace_blocks: Vec<bool>,
    trace_connections: Vec<bool>,
    query: String,
    matches: Vec<usize>,
    search_marks: Vec<bool>,
}

impl Viewer {
    pub fn load(path: &Path, cancel: &AtomicBool) -> Result<Self, String> {
        let graph = guarded(path, cancel, flowgraph::read)?;
        let layout = graph.layout();
        let selected = graph
            .connections
            .first()
            .map(|wire| wire.source.block)
            .or_else(|| (!graph.blocks.is_empty()).then_some(0));
        let block_count = graph.blocks.len();
        let connection_count = graph.connections.len();
        Ok(Self {
            graph,
            layout,
            selected,
            camera: Camera::default(),
            reveal: None,
            trace_direction: None,
            trace_steps: Vec::new(),
            trace_blocks: vec![false; block_count],
            trace_connections: vec![false; connection_count],
            query: String::new(),
            matches: Vec::new(),
            search_marks: vec![false; block_count],
        })
    }

    pub fn focus_block(&mut self, ordinal: u64) {
        if let Ok(block) = usize::try_from(ordinal)
            && block < self.graph.blocks.len()
        {
            self.select(Some(block), true);
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, close: &mut bool, palette: &AppPalette) {
        let colors = WorkbenchColors::from_palette(palette);
        let mut shell = crate::shell::StudioShell::new(
            "GNU Radio flowgraph",
            crate::icons::Icon::NodeTree,
            palette,
            close,
        );
        shell.render_header(ui, |_| {});
        self.toolbar(ui);
        let size = ui.available_size_before_wrap();
        if size.x >= 680.0 {
            let panel = (size.x * 0.32).clamp(250.0, 440.0);
            ui.horizontal(|ui| {
                self.canvas(
                    ui,
                    vec2((size.x - panel - 11.0).max(100.0), size.y.max(120.0)),
                    colors,
                );
                ui.allocate_ui_with_layout(
                    vec2(panel, size.y),
                    UiLayout::top_down(egui::Align::Min),
                    |ui| self.inspector(ui, size.y),
                );
            });
        } else {
            let canvas_height = (size.y * 0.57).max(100.0);
            self.canvas(ui, vec2(size.x, canvas_height), colors);
            self.inspector(ui, (size.y - canvas_height).max(76.0));
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.strong(&self.graph.title);
            ui.weak(format!(
                "{} blocks · {} connections",
                self.graph.blocks.len(),
                self.graph.connections.len()
            ));
            if let Some(version) = &self.graph.version {
                ui.weak(format!("GRC {version}"));
            } else {
                ui.weak(self.graph.format.label());
            }
            if self.graph.skipped_blocks > 0 || self.graph.skipped_connections > 0 {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    format!(
                        "{} unresolved entries",
                        self.graph.skipped_blocks + self.graph.skipped_connections
                    ),
                )
                .on_hover_text(format!(
                    "{} malformed or duplicate blocks · {} invalid or unresolved connections",
                    self.graph.skipped_blocks, self.graph.skipped_connections,
                ));
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button("Fit").clicked() {
                self.camera = Camera::default();
                self.reveal = None;
            }
            let query = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .id_salt("flowgraph-search")
                    .hint_text("Find block or type")
                    .char_limit(192)
                    .desired_width(170.0),
            );
            if query.changed() {
                self.search();
            }
            if query.lost_focus()
                && ui.input(|input| input.key_pressed(egui::Key::Enter))
                && let Some(&block) = self.matches.first()
            {
                self.select(Some(block), true);
            }
            ui.separator();
            let last = self.trace_direction;
            ui.selectable_value(&mut self.trace_direction, None, "Direct links");
            ui.selectable_value(
                &mut self.trace_direction,
                Some(Direction::Upstream),
                "Upstream",
            );
            ui.selectable_value(
                &mut self.trace_direction,
                Some(Direction::Downstream),
                "Downstream",
            );
            if self.trace_direction != last {
                self.refresh_trace();
            }
        });
    }

    fn canvas(&mut self, ui: &mut Ui, size: Vec2, colors: WorkbenchColors) {
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        if let Some(node) = self.reveal.take() {
            self.camera.reveal(rect, &self.layout, node);
        }
        if let Some(pointer) = response.hover_pos() {
            let scroll = ui.input(|input| input.smooth_scroll_delta().y);
            if scroll != 0.0 {
                self.camera.zoom_at(rect, &self.layout, pointer, scroll);
            }
        }
        if response.dragged() {
            self.camera.pan(
                self.camera.projection(rect, &self.layout),
                response.drag_delta(),
            );
        }
        let projection = self.camera.projection(rect, &self.layout);
        let scene = Scene {
            graph: &self.graph,
            layout: &self.layout,
        };
        let hovered = response
            .interact_pointer_pos()
            .or_else(|| response.hover_pos())
            .and_then(|point| scene.pick(projection, point));
        scene.draw(
            &ui.painter_at(rect),
            rect,
            projection,
            &Highlights {
                selected: self.selected,
                traced_blocks: &self.trace_blocks,
                traced_links: &self.trace_connections,
                search: &self.search_marks,
            },
            colors,
            false,
        );
        if response.clicked_by(PointerButton::Primary) {
            self.select(hovered, false);
            ui.ctx().request_repaint();
        }
        response.on_hover_ui(|ui| {
            if let Some(block) = hovered.and_then(|block| self.graph.blocks.get(block)) {
                ui.strong(&block.name);
                ui.monospace(&block.kind);
                ui.weak(block.state.label());
            } else {
                ui.weak("Click a block to inspect · Drag to pan · Scroll to zoom");
            }
        });
    }

    fn inspector(&mut self, ui: &mut Ui, height: f32) {
        let mut select = None;
        let pane = ("flowgraph-block", self.selected);
        ScrollArea::vertical()
            .id_salt(pane)
            .max_height(height)
            .show(ui, |ui| {
                if !self.query.trim().is_empty() {
                    ui.strong(format!("{} matches", self.matches.len()));
                    ScrollArea::vertical()
                        .id_salt("flowgraph-matches")
                        .max_height(130.0)
                        .show_rows(ui, 20.0, self.matches.len(), |ui, rows| {
                            for row in rows {
                                let index = self.matches[row];
                                let block = &self.graph.blocks[index];
                                if ui
                                    .selectable_label(self.selected == Some(index), &block.name)
                                    .on_hover_text(&block.kind)
                                    .clicked()
                                {
                                    select = Some(index);
                                }
                            }
                        });
                    ui.separator();
                }
                let Some(index) = self.selected else {
                    ui.strong("Flowgraph options");
                    show_settings(ui, &self.graph.options);
                    return;
                };
                let block = &self.graph.blocks[index];
                ui.strong(&block.name);
                ui.monospace(&block.kind);
                ui.horizontal_wrapped(|ui| {
                    ui.weak(block.state.label());
                    if let Some(degrees) = block.rotation.filter(|rotation| *rotation != 0) {
                        ui.weak(format!("{degrees}°"));
                    }
                });
                if !block.settings.is_empty() {
                    ui.separator();
                    show_settings(ui, &block.settings);
                }
                ui.separator();
                if self.trace_direction.is_some() {
                    self.trace_inspector(ui, &mut select);
                } else {
                    self.links_inspector(ui, index, &mut select);
                }
            });
        if let Some(block) = select {
            self.select(Some(block), true);
            ui.ctx().request_repaint();
        }
    }

    fn links_inspector(&self, ui: &mut Ui, index: usize, select: &mut Option<usize>) {
        for (direction, name) in [
            (Direction::Upstream, "Inputs"),
            (Direction::Downstream, "Outputs"),
        ] {
            let links = self.graph.adjacent(index, direction);
            ui.weak(format!("{} {name}", links.len()));
            for &edge in links.iter().take(128) {
                let wire = &self.graph.connections[edge];
                let (other, label) = match direction {
                    Direction::Upstream => (
                        wire.source.block,
                        format!(
                            "{} · {} → {}",
                            self.graph.blocks[wire.source.block].name,
                            wire.source.port,
                            wire.sink.port
                        ),
                    ),
                    Direction::Downstream => (
                        wire.sink.block,
                        format!(
                            "{} → {} · {}",
                            wire.source.port,
                            wire.sink.port,
                            self.graph.blocks[wire.sink.block].name
                        ),
                    ),
                };
                let item = ui.button(label);
                let item = if wire.settings.is_empty() {
                    item
                } else {
                    item.on_hover_ui(|ui| setting_rows(ui, &wire.settings))
                };
                if item.clicked() {
                    *select = Some(other);
                }
            }
            if links.len() > 128 {
                ui.weak(format!("First 128 of {} connected ports", links.len()));
            }
        }
    }

    fn trace_inspector(&self, ui: &mut Ui, select: &mut Option<usize>) {
        if self.trace_steps.is_empty() {
            ui.weak("No connected blocks in this direction");
        }
        let mut last_hops = 0;
        for step in self.trace_steps.iter() {
            if last_hops != step.hops {
                ui.weak(format!(
                    "{} {}",
                    step.hops,
                    if step.hops == 1 { "hop" } else { "hops" }
                ));
                last_hops = step.hops;
            }
            if ui.button(&self.graph.blocks[step.block].name).clicked() {
                *select = Some(step.block);
            }
        }
        if self.trace_steps.len() == TRACE_LIMIT {
            ui.weak(format!("First {TRACE_LIMIT} connected blocks"));
        }
    }

    fn select(&mut self, block: Option<usize>, reveal: bool) {
        self.selected = block;
        if reveal {
            self.reveal = block;
        }
        self.refresh_trace();
    }

    fn refresh_trace(&mut self) {
        self.trace_blocks.fill(false);
        self.trace_connections.fill(false);
        self.trace_steps = self
            .selected
            .zip(self.trace_direction)
            .map_or_else(Vec::new, |(start, direction)| {
                self.graph.trace(start, direction, TRACE_LIMIT)
            });
        for step in &self.trace_steps {
            self.trace_blocks[step.block] = true;
            self.trace_connections[step.connection] = true;
        }
    }

    fn search(&mut self) {
        self.search_marks.fill(false);
        self.matches.clear();
        let query = self.query.trim().to_lowercase();
        if query.is_empty() {
            return;
        }
        for (index, block) in self.graph.blocks.iter().enumerate() {
            if block.name.to_lowercase().contains(&query)
                || block.kind.to_lowercase().contains(&query)
            {
                self.search_marks[index] = true;
                self.matches.push(index);
            }
        }
    }
}

fn show_settings(ui: &mut Ui, settings: &[Setting]) {
    if settings.is_empty() {
        return;
    }
    if ui.button("Copy settings").clicked() {
        let mut output = String::new();
        for setting in settings {
            use std::fmt::Write as _;
            let suffix = if setting.shortened { "…" } else { "" };
            let _ = writeln!(output, "{} = {}{suffix}", setting.name, setting.value);
        }
        ui.ctx().copy_text(output);
    }
    setting_rows(ui, settings);
}

fn setting_rows(ui: &mut Ui, settings: &[Setting]) {
    for setting in settings {
        let boundary = setting
            .value
            .floor_char_boundary(setting.value.len().min(144));
        let abbreviated = boundary < setting.value.len() || setting.shortened;
        ui.horizontal_wrapped(|ui| {
            ui.weak(&setting.name);
            let value = &setting.value[..boundary];
            let value = if value.is_empty() { "—" } else { value };
            let response = ui.monospace(value);
            if abbreviated {
                response.on_hover_text(&setting.value);
                ui.weak("…");
            }
        });
    }
}

fn guarded<T>(
    path: &Path,
    cancel: &AtomicBool,
    read: impl FnOnce(std::fs::File, &AtomicBool) -> Result<T, String>,
) -> Result<T, String> {
    let check = ReadCheck::default();
    let (file, revision) = check.open(path, None, cancel)?;
    let loaded = read(file, cancel)?;
    revision.check()?;
    check.validate(cancel)?;
    Ok(loaded)
}

fn preview(path: &Path, cancel: &AtomicBool) -> Result<Preview, String> {
    guarded(path, cancel, flowgraph::preview)
}
