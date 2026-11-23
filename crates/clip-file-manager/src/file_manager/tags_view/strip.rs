use super::{Source, hex, plot};
use crate::{
    file_manager::{
        DocumentPosition, display_path,
        filesystem::Entry,
        shelf::preview::{Data, Ready},
        ui::{ListColumns, information_strip},
    },
    ui_appearance::WorkbenchColors,
};
use egui::{Id, Painter, Rect, Sense, Ui, pos2, vec2};
use little_clip_core::engineering::tags::{Document, Memory, RowRole, SampleBin, TraceBin};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

pub(in crate::file_manager) struct Preview {
    document: Document,
    chart: Chart,
}

impl Preview {
    pub(in crate::file_manager) fn document(&self) -> &Document {
        &self.document
    }
}

enum Chart {
    Memory(Vec<(f32, RowRole)>),
    Trace(Vec<TraceBin>),
    Signal(Vec<SampleBin>),
}

pub(in crate::file_manager) fn load(path: &Path, cancel: &AtomicBool) -> Data {
    let mut data = Data::new(
        display_path(path),
        "Tag recording".into(),
        crate::icons::Icon::Crosshair,
    );
    let document = match Source::open(path, cancel) {
        Ok((_, document)) => document,
        Err(error) => {
            data.detail = error;
            return data;
        }
    };
    let chart = match &document {
        Document::Memory(memory) => {
            let [known, total] = memory.coverage();
            data.detail = if total == 0 {
                format!(
                    "{} · {}-byte identifier",
                    memory.protocol,
                    memory.identifier.len()
                )
            } else {
                format!("{} · {known} / {total} bytes", memory.protocol)
            };
            Chart::Memory(memory_cells(memory))
        }
        Document::Trace(trace) => {
            data.detail = format!(
                "Proxmark trace · {}{} frames",
                trace.frames.len(),
                more(trace.complete)
            );
            Chart::Trace(plot::trace_preview(&trace.frames))
        }
        Document::Signal(signal) => {
            data.detail = format!(
                "GraphBuffer · {}{} samples",
                signal.samples.len(),
                more(signal.complete)
            );
            Chart::Signal(plot::sample_peaks(
                &signal.samples,
                signal.samples.len().div_ceil(256),
            ))
        }
    };
    data.tags = Some(Arc::new(Preview { document, chart }));
    data
}

fn more(complete: bool) -> &'static str {
    if complete { "" } else { "+" }
}

fn memory_cells(memory: &Memory) -> Vec<(f32, RowRole)> {
    let width = memory.rows.len().div_ceil(256).max(1);
    memory
        .rows
        .chunks(width)
        .map(|rows| {
            let [known, total] = rows.iter().fold([0, 0], |[known, total], row| {
                [known + row.known_bytes(), total + row.bytes.len()]
            });
            (known as f32 / total.max(1) as f32, rows[0].role)
        })
        .collect()
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
    let detail = ready.map_or("Reading tag recording…", |ready| {
        ready.data.detail.as_str()
    });
    let rect = information_strip(ui, row, entry, detail, Some(columns), colors);
    let Some(preview) = ready.and_then(|ready| ready.data.tags.as_ref()) else {
        return (None, false);
    };
    if preview.rows() == 0 {
        return (None, false);
    }
    let response = ui.interact(
        rect,
        Id::unique(("tag strip", &entry.path, entry.revision)),
        Sense::click(),
    );
    *row_response = row_response.clone().union(response.clone());
    let hovered = response
        .hover_pos()
        .or_else(|| response.interact_pointer_pos())
        .map(|pos| preview.pick(plot::fraction(rect, pos.x)));
    preview.draw(&ui.painter_at(rect), rect, colors, hovered);
    response.on_hover_ui(|ui| preview.tooltip(ui, colors, hovered));
    (
        hovered.map(|ordinal| DocumentPosition::TagRecord {
            ordinal,
            revision: entry.revision,
        }),
        false,
    )
}

impl Preview {
    fn rows(&self) -> u64 {
        match &self.document {
            Document::Memory(memory) => memory.rows.len() as u64,
            Document::Trace(trace) => trace.frames.len() as u64,
            Document::Signal(signal) => signal.samples.len() as u64,
        }
    }

    fn pick(&self, portion: f32) -> u64 {
        if let Chart::Trace(bins) = &self.chart {
            plot::TraceAxis::new(bins).pick(bins, portion)
        } else {
            let domain = 0..self.rows();
            plot::position(&domain, portion).min(domain.end - 1)
        }
    }

    fn draw(&self, painter: &Painter, rect: Rect, colors: WorkbenchColors, selected: Option<u64>) {
        let mut domain = 0..self.rows();
        let mut point = selected;
        match &self.chart {
            Chart::Memory(cells) => draw_memory(painter, rect, cells, colors),
            Chart::Trace(bins) => {
                let axis = plot::TraceAxis::new(bins);
                plot::trace(painter, rect, bins, &axis, colors);
                domain = axis.domain();
                point = selected.map(|row| axis.for_row(bins, row, None));
            }
            Chart::Signal(peaks) => {
                let stride = self.rows().div_ceil(256).max(1);
                plot::samples(painter, rect, &domain, 0, stride, peaks, colors);
            }
        }
        if let Some(row) = point {
            plot::marker(painter, rect, &domain, row, colors.focus);
        }
    }

    fn tooltip(&self, ui: &mut Ui, colors: WorkbenchColors, row: Option<u64>) {
        ui.set_width(352.0);
        ui.strong(self.document.format().title());
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 65.0), Sense::hover());
        self.draw(&ui.painter_at(rect), rect, colors, row);
        let index = row.unwrap_or(0) as usize;
        match &self.document {
            Document::Memory(memory) => {
                ui.strong(&memory.protocol);
                if let Some(row) = memory.rows.get(index) {
                    ui.weak(format!(
                        "{} · {} / {} bytes",
                        row.name,
                        row.known_bytes(),
                        row.bytes.len()
                    ));
                    ui.monospace(hex(row.bytes.iter().copied().take(16)));
                }
            }
            Document::Trace(trace) => {
                if let Some(frame) = trace.frames.get(index) {
                    ui.weak(format!(
                        "Frame {} · {} · {} ticks · {} bytes",
                        frame.row + 1,
                        plot::direction(frame.direction),
                        frame.timestamp_ticks,
                        frame.data_len,
                    ));
                }
            }
            Document::Signal(signal) => {
                if let Some(sample) = signal.samples.get(index) {
                    ui.monospace(format!("Sample {} · {sample}", index + 1));
                }
            }
        }
        ui.weak("Double-click to inspect");
    }
}

fn draw_memory(painter: &Painter, rect: Rect, cells: &[(f32, RowRole)], colors: WorkbenchColors) {
    painter.rect_filled(rect, 3.0, colors.recessed);
    let mut mesh = egui::Mesh::default();
    let width = rect.width() / cells.len().max(1) as f32;
    for (index, &(value, role)) in cells.iter().enumerate() {
        let left = rect.left() + index as f32 * width;
        let color = match role {
            RowRole::Manufacturer | RowRole::Rom => colors.link,
            RowRole::SectorTrailer => colors.warning,
            RowRole::Data => colors.accent,
        };
        mesh.add_colored_rect(
            Rect::from_min_max(
                pos2(left, rect.top() + 2.0),
                pos2(left + (width - 0.65).max(0.8), rect.bottom() - 2.0),
            ),
            color.gamma_multiply(0.15 + value * 0.85),
        );
    }
    painter.with_clip_rect(rect).add(mesh);
}
