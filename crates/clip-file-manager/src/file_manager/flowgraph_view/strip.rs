use super::{
    Selection,
    canvas::{Camera, Highlights, Scene},
    preview,
};
use crate::{
    file_manager::{
        DocumentPosition, display_path,
        filesystem::Entry,
        shelf::preview::{Data, Ready},
        ui::{ListColumns, information_strip},
    },
    ui_appearance::WorkbenchColors,
};
use egui::{Id, Rect, Sense, Ui, vec2};
use little_clip_core::engineering::flowgraph::{self, Direction};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

pub(in crate::file_manager) struct Preview {
    pub(in crate::file_manager) data: flowgraph::Preview,
}

pub(in crate::file_manager) fn load(path: &Path, cancel: &AtomicBool) -> Data {
    let mut data = Data::new(
        display_path(path),
        "GNU Radio flowgraph".into(),
        crate::icons::Icon::NodeTree,
    );
    match preview(path, cancel) {
        Ok(graph) => {
            data.detail = if graph.complete {
                format!(
                    "{} blocks · {} connections",
                    graph.graph.blocks.len(),
                    graph.graph.connections.len()
                )
            } else {
                format!("{} initial blocks", graph.graph.blocks.len())
            };
            data.flowgraph = Some(Arc::new(Preview { data: graph }));
        }
        Err(error) => data.detail = error,
    }
    data
}

pub(in crate::file_manager) fn show(
    ui: &mut Ui,
    row: Rect,
    entry: &Entry,
    columns: ListColumns,
    colors: WorkbenchColors,
    ready: Option<&Ready>,
    row_response: &mut egui::Response,
) -> (Option<DocumentPosition>, bool) {
    let detail = ready.map_or("Reading flowgraph…", |ready| ready.data.detail.as_str());
    let rect = information_strip(ui, row, entry, detail, Some(columns), colors);
    let Some(preview) = ready.and_then(|ready| ready.data.flowgraph.as_deref()) else {
        return (None, false);
    };
    let scene = preview.scene();
    let projection = Camera::default().projection(rect, &preview.data.layout);
    let response = ui.interact(
        rect,
        Id::unique(("grc-strip", &entry.path, entry.revision)),
        Sense::click(),
    );
    *row_response = row_response.clone().union(response.clone());
    let hovered = response
        .hover_pos()
        .or_else(|| response.interact_pointer_pos())
        .and_then(|point| scene.pick(projection, point));
    scene.draw(
        &ui.painter_at(rect),
        rect,
        projection,
        &Highlights {
            selected: hovered.map(Selection::Block),
            ..Default::default()
        },
        colors,
        true,
    );
    response.on_hover_ui(|ui| preview.expanded(ui, colors, hovered));
    (
        hovered.map(|block| DocumentPosition::FlowgraphBlock {
            ordinal: block as u64,
            revision: entry.revision,
        }),
        false,
    )
}

impl Preview {
    fn scene(&self) -> Scene<'_> {
        Scene {
            graph: &self.data.graph,
            layout: &self.data.layout,
        }
    }

    fn expanded(&self, ui: &mut Ui, colors: WorkbenchColors, hovered: Option<usize>) {
        let graph = &self.data.graph;
        ui.set_width(370.0);
        ui.strong(&graph.title);
        ui.weak(graph.format.label());
        if !self.data.complete {
            ui.weak("Showing the initial flowgraph");
        }
        if let Some(expression) = graph.sample_rate() {
            let text = &expression[..expression.floor_char_boundary(expression.len().min(60))];
            ui.monospace(format!("Sample rate  {text}"));
        }
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 158.0), Sense::hover());
        let projection = Camera::default().projection(rect, &self.data.layout);
        self.scene().draw(
            &ui.painter_at(rect),
            rect,
            projection,
            &Highlights {
                selected: hovered.map(Selection::Block),
                ..Default::default()
            },
            colors,
            false,
        );
        if let Some(index) = hovered {
            let block = &graph.blocks[index];
            ui.horizontal_wrapped(|ui| {
                ui.strong(&block.name);
                ui.monospace(&block.kind);
                ui.weak(block.state.label());
            });
            ui.weak(format!(
                "{} inputs · {} outputs",
                graph.adjacent(index, Direction::Upstream).len(),
                graph.adjacent(index, Direction::Downstream).len(),
            ));
        }
    }
}
