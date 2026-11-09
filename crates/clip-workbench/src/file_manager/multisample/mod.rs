//! Shared key/velocity maps and explicit raw-sample audition for multisample formats.
mod archive;
mod audition;
mod nki;
mod sfz_source;
use super::{filesystem::Entry, media::PreviewData, preview_job::Task, shelf::preview::Ready};
use crate::{
    icons::Icon,
    shell::{StudioPanelLayout, StudioShell},
    ui_appearance::WorkbenchColors,
};
use little_clip_core::{
    BackgroundJobs,
    multisample::{Document, Region},
    palette::AppPalette,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
pub(super) fn known(path: &Path) -> bool {
    super::document_format::Format::for_path(path)
        == Some(super::document_format::Format::Multisample)
}
fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
}
struct Mapping {
    document: Document,
    archive: Option<archive::Archive>,
}
fn read(path: &Path, token: &AtomicBool) -> Result<Mapping, String> {
    let ext = extension(path).ok_or("Missing multisample extension")?;
    if matches!(ext.as_str(), "multisample" | "dslibrary" | "xrni") {
        let (document, archive) = archive::read(path, token)?;
        return Ok(Mapping {
            document,
            archive: Some(archive),
        });
    }
    Ok(Mapping {
        document: read_external(path, &ext, token)?,
        archive: None,
    })
}
fn read_external(path: &Path, ext: &str, token: &AtomicBool) -> Result<Document, String> {
    if ext == "sfz" {
        little_clip_core::multisample::sfz::parse(sfz_source::read(path, token)?.as_bytes())
    } else {
        let limit = match ext {
            "sf2" => 256 * 1024 * 1024,
            "dspreset" | "xml" => 4 * 1024 * 1024,
            _ => 64 * 1024 * 1024,
        };
        let bytes = crate::file_source::read_file_snapshot(path, limit)?;
        if token.load(Ordering::Relaxed) {
            return Err("Multisample opening cancelled".into());
        }
        match ext {
            "sf2" => little_clip_core::multisample::sf2::parse(bytes.into()),
            "nki" | "exs" => nki::read(bytes, token),
            "dspreset" | "xml" => little_clip_core::multisample::xml::parse(
                &bytes,
                path.file_stem()
                    .and_then(|n| n.to_str())
                    .unwrap_or("Preset"),
            ),
            _ => return Err("Unsupported multisample format".into()),
        }
    }
}
/// Ambiguous names retain their usual viewer unless sampler contents are identified.
pub(super) fn read_if_identified(path: &Path, token: &AtomicBool) -> Option<Document> {
    let ext = extension(path)?;
    match ext.as_str() {
        "xml" => {}
        "exs" => {
            let prefix = crate::file_source::read_file_bytes(path, 84, false, true).ok()?;
            if !matches!(prefix.get(..3), Some([0, 1, 0] | [1, 1, 0])) {
                return None;
            }
        }
        _ => return None,
    }
    let mapping = read(path, token).ok()?;
    mapping
        .document
        .instruments
        .iter()
        .any(|i| !i.regions.is_empty())
        .then_some(mapping.document)
}
pub(super) fn load(path: &Path, token: &AtomicBool) -> PreviewData {
    match read(path, token) {
        Ok(mapping) => preview(mapping.document),
        Err(error) => PreviewData::new(String::new(), error, Icon::Midi),
    }
}
pub(super) fn preview(mut document: Document) -> PreviewData {
    let mut data = PreviewData::new(String::new(), "Multisample".into(), Icon::Midi);
    let regions: usize = document.instruments.iter().map(|i| i.regions.len()).sum();
    data.detail = format!(
        "{} instruments · {regions} regions · partial mapping preview",
        document.instruments.len()
    );
    document.pcm = None;
    document.notes.clear();
    document.instruments.truncate(16);
    let mut remaining = 256;
    for instrument in &mut document.instruments {
        instrument.regions.truncate(remaining);
        remaining -= instrument.regions.len();
        instrument.name = instrument.name.chars().take(80).collect();
        for region in &mut instrument.regions {
            region.name = region.name.chars().take(80).collect();
            region.sample.clear();
            region.properties.clear();
            region.loops.clear();
            region.embedded = None;
        }
    }
    data.multisample = Some(Arc::new(document));
    data
}
pub(super) struct Viewer {
    document: Arc<Document>,
    base: PathBuf,
    archive: Option<archive::Archive>,
    instrument: usize,
    selected: usize,
    task: Option<Task<Result<audition::Audio, String>>>,
    audio: Option<audition::Audio>,
    player: Option<rodio::Player>,
    error: Option<String>,
}
impl Viewer {
    pub fn load(path: &Path, token: &AtomicBool) -> Result<Self, String> {
        Ok(Self::from_mapping(path, read(path, token)?))
    }
    pub(super) fn from_identified(path: &Path, document: Document) -> Self {
        Self::from_mapping(
            path,
            Mapping {
                document,
                archive: None,
            },
        )
    }
    fn from_mapping(path: &Path, mapping: Mapping) -> Self {
        Self {
            document: Arc::new(mapping.document),
            archive: mapping.archive,
            base: path.parent().unwrap_or(Path::new(".")).into(),
            instrument: 0,
            selected: 0,
            task: None,
            audio: None,
            player: None,
            error: None,
        }
    }
    pub fn relocate_file(&mut self, source: crate::file_source::FileSource) {
        if let Some(archive) = &mut self.archive {
            self.task = None;
            self.error = None;
            archive.relocate(source);
        }
    }
    pub fn select(&mut self, instrument: usize, region: usize) {
        if self
            .document
            .instruments
            .get(instrument)
            .is_some_and(|i| region < i.regions.len())
        {
            self.stop();
            self.audio = None;
            self.instrument = instrument;
            self.selected = region;
            self.error = None;
        }
    }
    fn current(&self) -> Option<&Region> {
        self.document
            .instruments
            .get(self.instrument)?
            .regions
            .get(self.selected)
    }
    fn stop(&mut self) {
        if let Some(player) = self.player.take() {
            player.stop();
        }
        self.task = None;
    }
    fn play(&mut self) {
        self.stop();
        self.error = None;
        if let Some(audio) = &self.audio {
            if let Some(player) = crate::ui_ext::new_shared_audio_player() {
                player.append(audio.source.clone());
                self.player = Some(player);
            } else {
                self.error = Some("Cannot open audio output".into());
            }
        } else if let Some(region) = self.current().cloned() {
            let document = self.document.clone();
            let base = self.base.clone();
            let archive = self.archive.clone();
            self.task = Some(Task::start(
                BackgroundJobs::global(),
                Duration::from_secs(35),
                move |token| audition::prepare(&document, &region, &base, archive.as_ref(), token),
            ));
        }
    }
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.task.as_ref().and_then(Task::poll) {
            self.task = None;
            match result {
                Ok(Ok(audio)) => {
                    self.audio = Some(audio);
                    self.play();
                }
                Ok(Err(e)) => self.error = Some(e),
                Err(e) => self.error = Some(e.to_string()),
            }
        }
        if self.player.as_ref().is_some_and(|p| p.empty()) {
            self.player = None;
        }
        if self.task.is_some() || self.player.is_some() {
            ctx.request_repaint_after(Duration::from_millis(40));
        }
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, close: &mut bool, palette: &AppPalette) {
        self.poll(ui.ctx());
        StudioShell::new("Multisample", Icon::Midi, palette, close).render_header(ui, |_| {});
        let layout = StudioPanelLayout::default().with_left().allocate(ui);
        if let Some(left) = layout.left {
            ui.allocate_ui_at_rect(left, |ui| {
                ui.set_clip_rect(left.intersect(ui.clip_rect()));
                self.details(ui);
            });
        }
        ui.allocate_ui_at_rect(layout.center, |ui| {
            ui.set_clip_rect(layout.center.intersect(ui.clip_rect()));
            let colors = WorkbenchColors::from_palette(palette);
            let Some(instrument) = self.document.instruments.get(self.instrument) else {
                ui.label("No instruments in this file");
                return;
            };
            ui.label("Key / velocity map · click a region to inspect its source");
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(
                    ui.available_width(),
                    (ui.available_height() - 125.0).max(40.0),
                ),
                egui::Sense::click(),
            );
            let hit = paint(ui, rect, &instrument.regions, Some(self.selected), colors);
            if response.clicked() {
                if let Some(region) = hit {
                    self.select(self.instrument, region);
                }
            }
            ui.label("MIDI keys 0–127 (C−1 to G9) · velocity increases upwards");
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        self.current().is_some() && self.task.is_none(),
                        egui::Button::new("Audition raw sample"),
                    )
                    .clicked()
                {
                    self.play();
                }
                if ui
                    .add_enabled(
                        self.player.is_some() || self.task.is_some(),
                        egui::Button::new("Stop"),
                    )
                    .clicked()
                {
                    self.stop();
                }
                if self.task.is_some() {
                    ui.spinner();
                }
                if let Some(audio) = &self.audio {
                    ui.label(format!(
                        "{:.2} s · up to 10 s, no pitch/DSP processing",
                        audio.seconds
                    ));
                }
            });
            if let Some(error) = &self.error {
                ui.colored_label(colors.danger, error);
            }
            if let Some(audio) = &self.audio {
                let (wave, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), ui.available_height().max(1.0)),
                    egui::Sense::hover(),
                );
                let painter = ui.painter().with_clip_rect(wave.intersect(ui.clip_rect()));
                painter.rect_filled(wave, 3.0, colors.recessed);
                for (index, peak) in audio.peaks.iter().enumerate() {
                    let x = wave.left()
                        + wave.width() * (index as f32 + 0.5) / audio.peaks.len() as f32;
                    let h = peak * wave.height() * 0.45;
                    painter.line_segment(
                        [
                            egui::pos2(x, wave.center().y - h),
                            egui::pos2(x, wave.center().y + h),
                        ],
                        egui::Stroke::new(1.0_f32, colors.accent),
                    );
                }
            }
        });
    }
    fn details(&mut self, ui: &mut egui::Ui) {
        ui.label(format!("{} instruments", self.document.instruments.len()));
        let mut instrument = self.instrument;
        egui::ComboBox::from_id_source("multisample-instrument")
            .width(ui.available_width() - 10.0)
            .selected_text(
                self.document
                    .instruments
                    .get(instrument)
                    .map_or("No instrument", |i| i.name.as_str()),
            )
            .show_ui(ui, |ui| {
                for (index, item) in self.document.instruments.iter().enumerate() {
                    ui.selectable_value(&mut instrument, index, &item.name);
                }
            });
        if instrument != self.instrument {
            self.stop();
            self.audio = None;
            self.instrument = instrument;
            self.selected = 0;
            self.error = None;
        }
        let Some(instrument) = self.document.instruments.get(self.instrument) else {
            return;
        };
        ui.label(format!("{} regions", instrument.regions.len()));
        let height = crate::ui_ext::selectable_text_row_height(ui, 28.0);
        let mut selected = self.selected;
        egui::ScrollArea::vertical()
            .id_source("multisample-regions")
            .max_height((ui.available_height() * 0.28).max(height))
            .min_scrolled_height(0.0)
            .show_rows(ui, height, instrument.regions.len(), |ui, rows| {
                for index in rows {
                    if crate::ui_ext::selectable_text_row(
                        ui,
                        &instrument.regions[index].name,
                        index == selected,
                        height,
                    )
                    .clicked()
                    {
                        selected = index;
                    }
                }
            });
        if selected != self.selected {
            self.select(self.instrument, selected);
        }
        ui.separator();
        egui::ScrollArea::vertical()
            .id_source("multisample-properties")
            .auto_shrink([false, false])
            .min_scrolled_height(0.0)
            .show(ui, |ui| {
                if let Some(region) = self.current() {
                    ui.strong(&region.name);
                    ui.label(format!(
                        "Keys {}–{} · root {}",
                        region.key[0], region.key[1], region.root
                    ));
                    ui.label(format!(
                        "Velocity {}–{}",
                        region.velocity[0], region.velocity[1]
                    ));
                    ui.add(egui::Label::new(&region.sample).wrap(true));
                    ui.label(format!("Sample start {} frames", region.start));
                    if let Some(end) = region.end {
                        ui.label(format!("Sample end {end} (exclusive)"));
                    }
                    for [start, end] in &region.loops {
                        ui.label(format!("Loop {start}–{end} (exclusive)"));
                    }
                    for (key, value) in &region.properties {
                        ui.strong(key);
                        ui.add(egui::Label::new(value).wrap(true));
                    }
                }
                for note in &self.document.notes {
                    ui.add(egui::Label::new(note).wrap(true));
                }
            });
    }
}
impl Drop for Viewer {
    fn drop(&mut self) {
        self.stop();
    }
}
fn paint(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    regions: &[Region],
    selected: Option<usize>,
    colors: WorkbenchColors,
) -> Option<usize> {
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 3.0, colors.recessed);
    let pointer = super::ui::preview_hovered(ui, rect)
        .then(|| ui.input(|i| i.pointer.hover_pos()))
        .flatten();
    for key in (0..128).step_by(12) {
        let x = rect.left() + rect.width() * key as f32 / 128.0;
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(1.0_f32, colors.text_muted.gamma_multiply(0.4)),
        );
    }
    let mut hit = None;
    for (index, region) in regions.iter().enumerate() {
        let box_rect = egui::Rect::from_min_max(
            egui::pos2(
                rect.left() + rect.width() * region.key[0] as f32 / 128.0,
                rect.bottom() - rect.height() * (region.velocity[1] as f32 + 1.0) / 128.0,
            ),
            egui::pos2(
                rect.left() + rect.width() * (region.key[1] as f32 + 1.0) / 128.0,
                rect.bottom() - rect.height() * region.velocity[0] as f32 / 128.0,
            ),
        );
        let color = if selected == Some(index) {
            colors.accent
        } else {
            colors.text_secondary
        };
        painter.rect_filled(box_rect, 1.0, color.gamma_multiply(0.22));
        painter.rect_stroke(
            box_rect,
            1.0,
            egui::Stroke::new(
                if selected == Some(index) {
                    2.0_f32
                } else {
                    1.0_f32
                },
                color,
            ),
        );
        if box_rect.width() > 80.0 && box_rect.height() > 24.0 {
            painter.with_clip_rect(box_rect.intersect(rect)).text(
                box_rect.left_top() + egui::vec2(4.0, 4.0),
                egui::Align2::LEFT_TOP,
                &region.name,
                egui::TextStyle::Small.resolve(ui.style()),
                colors.text_primary,
            );
        }
        if pointer.is_some_and(|p| box_rect.contains(p)) {
            hit = Some(index);
        }
    }
    if let Some(index) = hit {
        egui::show_tooltip_at_pointer(ui.ctx(), egui::Id::new("sample-region"), |ui| {
            ui.label(&regions[index].name);
            ui.label(format!(
                "Keys {:?} · velocity {:?}",
                regions[index].key, regions[index].velocity
            ));
        });
    }
    hit
}
pub(super) fn show_list(
    ui: &mut egui::Ui,
    row: egui::Rect,
    entry: &Entry,
    columns: super::ui::ListColumns,
    ready: Option<&Ready>,
    colors: WorkbenchColors,
) -> Option<(usize, usize)> {
    let rect = super::ui::information_strip(
        ui,
        row,
        entry,
        ready.map_or("Sample mappings", |r| &r.data.detail),
        Some(columns),
        colors,
    );
    let doc = ready.and_then(|r| r.data.multisample.as_ref())?;
    let instrument = doc.instruments.first()?;
    paint(ui, rect, &instrument.regions, None, colors).map(|region| (0, region))
}

#[cfg(test)]
mod tests;
