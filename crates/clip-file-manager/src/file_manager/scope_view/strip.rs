use super::{copy_samples, plot};
use crate::file_manager::{
    DocumentPosition, display_path,
    filesystem::Entry,
    preview_copy,
    search::PreviewContext,
    shelf::preview::{Data, Ready},
    ui::{ListColumns, information_strip},
};
use crate::file_source::{FileSource, ReadCheck};
use crate::ui_appearance::WorkbenchColors;
use egui::{Color32, Id, Rect, Sense, Ui};
use little_clip_core::engineering::waveforms::{Overview, Recording, display_value};
use std::{
    ops::Range,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

pub(in crate::file_manager) struct Preview {
    source: FileSource,
    overview: Overview,
}

impl Preview {
    pub(in crate::file_manager) fn overview(&self) -> &Overview {
        &self.overview
    }
}

#[derive(Clone)]
pub(in crate::file_manager) struct Selection {
    pub path: std::path::PathBuf,
    revision: u64,
    preview: Arc<Preview>,
    samples: Range<u64>,
}

impl Selection {
    pub fn export(&self, context: &PreviewContext, cancel: &AtomicBool) -> Result<String, String> {
        context.read(&self.path, cancel, || {
            let recording = Recording::open_expected(
                &self.path,
                Some(&self.preview.source),
                ReadCheck::default(),
                cancel,
            )?;
            let channel = &self.preview.overview.channels[0];
            copy_samples(
                &recording,
                0,
                channel.time_at(self.samples.start)..channel.time_at(self.samples.end),
                cancel,
            )
        })
    }

    fn position(&self) -> DocumentPosition {
        DocumentPosition::ScopeSamples {
            first: self.samples.start,
            end: self.samples.end,
            revision: self.revision,
        }
    }

    fn matches(&self, entry: &Entry) -> bool {
        self.path == entry.path && self.revision == entry.revision
    }
}

pub(in crate::file_manager) fn focus_id() -> Id {
    Id::unique("file_scope_strip_focus")
}

fn entry_is_power(path: &Path) -> bool {
    use little_clip_core::file_format::Format;
    Format::for_path(path) == Some(Format::PowerRecording)
}

pub(in crate::file_manager) fn load(path: &Path, cancel: &AtomicBool) -> Data {
    let power = entry_is_power(path);
    let mut data = Data::new(
        display_path(path),
        if power {
            "Power recording"
        } else {
            "Oscilloscope capture"
        }
        .into(),
        if power {
            crate::icons::Icon::Zap
        } else {
            crate::icons::Icon::Waves
        },
    );
    let preview = Recording::open(path, ReadCheck::default(), cancel).and_then(|recording| {
        let overview = recording.preview(cancel)?;
        Ok(Preview {
            source: recording.source().clone(),
            overview,
        })
    });
    match preview {
        Ok(preview) => {
            let overview = &preview.overview;
            data.detail = format!(
                "{} · {} ch · {}{}",
                overview.model,
                overview.channels.len(),
                display_value(overview.duration_seconds(), "s"),
                if overview.sampled() {
                    " · sampled"
                } else {
                    ""
                },
            );
            data.scope = Some(Arc::new(preview));
        }
        Err(error) => data.detail = error,
    }
    data
}

#[allow(clippy::too_many_arguments)]
pub(in crate::file_manager) fn show(
    ui: &mut Ui,
    row: Rect,
    entry: &Entry,
    columns: ListColumns,
    colors: WorkbenchColors,
    ready: Option<&Ready>,
    selected: &mut Option<Selection>,
    row_response: &mut egui::Response,
) -> (Option<DocumentPosition>, bool) {
    let detail = ready.map_or("Reading waveform…", |ready| ready.data.detail.as_str());
    let rect = information_strip(ui, row, entry, detail, Some(columns), colors);
    let Some(preview) = ready.and_then(|ready| ready.data.scope.as_ref()) else {
        return (None, false);
    };
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
    plot::overview(
        &painter,
        rect.shrink2(egui::vec2(2.0, 1.0)),
        &preview.overview,
    );
    let response = ui.interact(
        rect,
        Id::unique(("scope_strip", &entry.path)),
        Sense::click_and_drag(),
    );
    *row_response = row_response.clone().union(response.clone());
    let modified = ui.input(|input| input.modifiers.command || input.modifiers.alt);
    let (engaged, drag) = preview_copy::drag_range(ui, &response, focus_id(), modified);
    let channel = &preview.overview.channels[0];
    let index = |fraction: f32| {
        (f64::from(fraction) * channel.sample_count as f64)
            .round()
            .clamp(0.0, channel.sample_count as f64) as u64
    };
    if let Some((left, right)) = drag {
        let left = index(left).min(channel.sample_count.saturating_sub(1));
        let right = index(right).max(left + 1).min(channel.sample_count);
        *selected = Some(Selection {
            path: entry.path.clone(),
            revision: entry.revision,
            preview: preview.clone(),
            samples: left..right,
        });
    }
    let selection = selected
        .as_ref()
        .filter(|selection| selection.matches(entry));
    if let Some(selection) = selection {
        let count = channel.sample_count as f64;
        plot::highlight(
            &painter,
            rect,
            &(0.0..count),
            selection.samples.start as f64..selection.samples.end as f64,
            Color32::from_white_alpha(61),
        );
    }
    let seek = response.hover_pos().map(|point| {
        let sample = index(((point.x - rect.left()) / rect.width()).clamp(0.0, 1.0));
        let half = (channel.sample_count / 20).max(1);
        DocumentPosition::ScopeSamples {
            first: sample.saturating_sub(half),
            end: (sample + half).min(channel.sample_count),
            revision: entry.revision,
        }
    });
    response.on_hover_ui(|ui| {
        let overview = &preview.overview;
        ui.strong(&overview.model);
        ui.horizontal_wrapped(|ui| {
            for (index, channel) in overview.channels.iter().enumerate() {
                ui.colored_label(
                    plot::color(index),
                    format!("{} ({})", channel.name, channel.unit),
                );
            }
        });
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(350.0, 32.0 * overview.channels.len() as f32),
            Sense::hover(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
        plot::overview(&painter, rect.shrink(4.0), overview);
        if selection.is_some() {
            ui.weak("Ctrl+C copies exact first-channel samples as CSV");
        } else {
            ui.weak("Drag to select · Double-click to open the region");
        }
    });
    (selection.map(Selection::position).or(seek), engaged)
}
