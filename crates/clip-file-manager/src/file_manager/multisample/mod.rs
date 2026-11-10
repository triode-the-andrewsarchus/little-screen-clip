//! Shared key/velocity maps and explicit raw-sample audition for multisample formats.
mod archive;
mod audition;
mod explorer;
mod midi;
mod nki;
mod sfz_source;
mod strip;
mod surface;
use super::{media::PreviewData, preview_job::Task};
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
pub(super) use strip::Preview;
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
    strike: Option<surface::Strike>,
    velocity: u8,
    pitched: bool,
    explorer: explorer::Explorer,
    chord: Vec<surface::Strike>,
    chord_regions: Vec<usize>,
    midi: midi::Performance,
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
        let explorer = explorer::Explorer::new(
            mapping
                .document
                .instruments
                .first()
                .map_or(&[], |i| &i.regions),
        );
        Self {
            explorer,
            midi: Default::default(),
            chord: Vec::new(),
            chord_regions: Vec::new(),
            document: Arc::new(mapping.document),
            archive: mapping.archive,
            base: path.parent().unwrap_or(Path::new(".")).into(),
            instrument: 0,
            selected: 0,
            task: None,
            audio: None,
            player: None,
            error: None,
            strike: None,
            velocity: 96,
            pitched: true,
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
            self.chord.clear();
            self.chord_regions.clear();
            self.audio = None;
            if instrument != self.instrument {
                self.midi = Default::default();
                self.explorer =
                    explorer::Explorer::new(&self.document.instruments[instrument].regions);
            }
            self.instrument = instrument;
            self.selected = region;
            let region = self.current().unwrap();
            self.strike = Some(surface::Strike {
                key: region.root.clamp(region.key[0], region.key[1]),
                velocity: 96_u8.clamp(region.velocity[0].max(1), region.velocity[1].max(1)),
            });
            self.velocity = self.strike.unwrap().velocity;
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
        self.midi.stop();
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
                use rodio::Source;
                let speed = if self.pitched && self.chord.len() <= 1 {
                    self.strike
                        .zip(self.current())
                        .map_or(1.0, |(strike, region)| {
                            2.0_f32.powf((strike.key as f32 - region.root as f32) / 12.0)
                        })
                } else {
                    1.0
                };
                player.append(
                    audio
                        .source
                        .clone()
                        .speed(speed)
                        .amplify(if self.chord.len() > 1 {
                            1.0
                        } else {
                            audition::velocity_gain(self.velocity)
                        })
                        .take_duration(Duration::from_secs(10)),
                );
                self.player = Some(player);
            } else {
                self.error = Some("Cannot open audio output".into());
            }
        } else if let Some(region) = self.current().cloned() {
            let document = self.document.clone();
            let base = self.base.clone();
            let archive = self.archive.clone();
            let chord = self.chord.clone();
            let instrument = self.instrument;
            let indices = self.chord_regions.clone();
            self.task = Some(Task::start(
                BackgroundJobs::global(),
                Duration::from_secs(35),
                move |token| {
                    if chord.len() > 1 {
                        audition::prepare_chord(
                            &document,
                            instrument,
                            &indices,
                            &chord,
                            &base,
                            archive.as_ref(),
                            token,
                        )
                    } else {
                        audition::prepare(&document, &region, &base, archive.as_ref(), token)
                    }
                },
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
    fn trigger(&mut self, strike: surface::Strike) {
        if !self.chord.is_empty() {
            self.stop();
            self.audio = None;
            self.chord.clear();
            self.chord_regions.clear();
        }
        let Some(instrument) = self.document.instruments.get(self.instrument) else {
            return;
        };
        let previous = (self.strike == Some(strike)).then_some(self.selected);
        let chosen = surface::choose(&instrument.regions, strike, previous);
        self.strike = Some(strike);
        self.velocity = strike.velocity;
        if let Some(index) = chosen {
            if index != self.selected {
                self.select(self.instrument, index);
            }
            self.strike = Some(strike);
            self.velocity = strike.velocity;
            if self.task.is_none() {
                self.play();
            }
        } else {
            self.stop();
            self.audio = None;
            self.selected = usize::MAX;
            self.error = Some(format!(
                "No recording at {} / velocity {}",
                surface::note(strike.key),
                strike.velocity
            ));
        }
    }
    fn trigger_chord(&mut self, root: surface::Strike) {
        self.pitched = true;
        let Some(notes) = self.explorer.chord(root.key) else {
            self.stop();
            self.audio = None;
            self.selected = usize::MAX;
            self.error = Some("Voicing extends beyond MIDI range".into());
            return;
        };
        if notes.len() == 1 {
            self.trigger(root);
            return;
        }
        let strikes: Vec<_> = notes
            .iter()
            .map(|&key| surface::Strike {
                key,
                velocity: root.velocity,
            })
            .collect();
        let document = self.document.clone();
        let regions = &document.instruments[self.instrument].regions;
        let indices: Option<Vec<_>> = strikes
            .iter()
            .map(|&strike| surface::choose(regions, strike, None))
            .collect();
        self.stop();
        self.audio = None;
        self.strike = Some(root);
        self.chord.clear();
        self.chord_regions.clear();
        if let Some(indices) = indices {
            self.selected = indices[0];
            self.chord_regions = indices;
            self.chord = strikes;
            self.play();
        } else {
            self.selected = usize::MAX;
            let missing = strikes
                .iter()
                .filter(|&&strike| surface::choose(regions, strike, None).is_none())
                .map(|s| surface::note(s.key))
                .collect::<Vec<_>>()
                .join(", ");
            self.error = Some(format!(
                "No recording for {missing} at velocity {}",
                root.velocity
            ));
        }
    }
    fn sounding_keys(&self) -> Vec<u8> {
        let mut keys = self.midi.keys();
        if self.player.is_none() {
            return keys;
        }
        if self.chord.is_empty() {
            keys.extend(self.strike.iter().map(|s| s.key));
            keys
        } else {
            keys.extend(self.chord.iter().map(|s| s.key));
            keys
        }
    }
    fn poll_midi(&mut self, ctx: &egui::Context) {
        self.midi.poll(
            ctx,
            &self.document,
            self.instrument,
            &self.base,
            self.archive.as_ref(),
        );
        if let Some((strike, index)) = self.midi.latest.take() {
            if index != self.selected || !self.chord.is_empty() {
                self.audio = None;
            }
            self.chord.clear();
            self.chord_regions.clear();
            self.strike = Some(strike);
            self.velocity = strike.velocity;
            self.selected = index;
        }
        if self.midi.active() {
            self.task = None;
            if let Some(player) = self.player.take() {
                player.stop();
            }
        }
    }
    fn layers(&mut self, ui: &mut egui::Ui) {
        let Some(strike) = self.strike else {
            return;
        };
        ui.strong(format!(
            "{} · velocity {}",
            surface::note(strike.key),
            strike.velocity
        ));
        ui.small("Velocity layers · click to hear a recording");
        let mut chosen = None;
        if let Some(instrument) = self.document.instruments.get(self.instrument) {
            egui::ScrollArea::vertical()
                .id_source("velocity-layers")
                .max_height(130.0)
                .show(ui, |ui| {
                    for (index, region) in instrument
                        .regions
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| (r.key[0]..=r.key[1]).contains(&strike.key))
                    {
                        if ui
                            .selectable_label(
                                index == self.selected,
                                format!(
                                    "{}–{}  {}",
                                    region.velocity[0],
                                    region.velocity[1],
                                    region_label(region)
                                ),
                            )
                            .clicked()
                        {
                            chosen = Some((
                                index,
                                region.velocity[0]
                                    .saturating_add((region.velocity[1] - region.velocity[0]) / 2)
                                    .max(1),
                            ));
                        }
                    }
                });
        }
        if let Some((index, velocity)) = chosen {
            self.select(self.instrument, index);
            self.strike = Some(surface::Strike {
                key: strike.key,
                velocity,
            });
            self.velocity = velocity;
            self.play();
        }
    }
    pub fn ui(&mut self, ui: &mut egui::Ui, close: &mut bool, palette: &AppPalette) {
        self.poll(ui.ctx());
        self.poll_midi(ui.ctx());
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
            self.explorer_ui(ui, palette);
            self.playback_ui(ui, colors);
        });
    }
    fn explorer_ui(&mut self, ui: &mut egui::Ui, palette: &AppPalette) {
        if self.explorer.tabs(ui, palette) {
            self.stop();
            self.audio = None;
            self.chord.clear();
            self.chord_regions.clear();
            self.strike = None;
        }
        let document = self.document.clone();
        let Some(instrument) = document.instruments.get(self.instrument) else {
            ui.label("No instruments in this file");
            return;
        };
        self.performance_controls(ui);
        if self.explorer.mode == explorer::Mode::Harmony {
            let previous = (
                self.explorer.quality,
                self.explorer.voicing,
                self.explorer.center,
                self.explorer.seventh,
                self.explorer.lower,
                self.explorer.upper,
            );
            self.explorer.controls(ui);
            let current = (
                self.explorer.quality,
                self.explorer.voicing,
                self.explorer.center,
                self.explorer.seventh,
                self.explorer.lower,
                self.explorer.upper,
            );
            if previous != current
                && let Some(strike) = self.strike
            {
                let key = (i16::from(strike.key) + i16::from(current.2) - i16::from(previous.2))
                    .clamp(0, 127) as u8;
                self.trigger_chord(surface::Strike {
                    key,
                    velocity: self.velocity,
                });
            }
        }
        let hint = match self.explorer.mode {
            explorer::Mode::Harmony => {
                "Fifths → · major thirds ↘ · click / drag chords · wheel: zoom · right-drag: pan"
            }
            explorer::Mode::Pads => "Click a named recording to audition its original pitch",
            explorer::Mode::Mapping => "Keys → · velocity ↑ · click / drag to hear the mapping",
        };
        ui.small(hint);
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(
                ui.available_width(),
                (ui.available_height() - 145.0).max(60.0),
            ),
            egui::Sense::hover(),
        );
        let regions = &instrument.regions;
        let range = surface::bounds(regions);
        let sounding = self.sounding_keys();
        let hit = match self.explorer.mode {
            explorer::Mode::Harmony => {
                self.explorer
                    .harmony(ui, rect, regions, self.velocity, &sounding, palette)
            }
            explorer::Mode::Mapping => explorer::mapping(ui, rect, regions, self.strike, palette),
            explorer::Mode::Pads => {
                self.pads(ui, rect, palette);
                None
            }
        };
        if let Some(strike) = hit {
            if self.explorer.mode == explorer::Mode::Harmony {
                self.trigger_chord(strike);
            } else {
                self.trigger(strike);
            }
        }
        let (piano, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 38.0), egui::Sense::hover());
        if let Some(key) = surface::keyboard(ui, piano, range, &sounding, regions, palette) {
            self.trigger(surface::Strike {
                key,
                velocity: self.velocity,
            });
        }
    }
    fn performance_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let response = ui.add(egui::Slider::new(&mut self.velocity, 1..=127).text("Velocity"));
            if (response.drag_stopped()
                || (response.changed() && !ui.input(|i| i.pointer.primary_down())))
                && let Some(strike) = self.strike
            {
                let strike = surface::Strike {
                    velocity: self.velocity,
                    ..strike
                };
                if self.chord.len() > 1 {
                    self.trigger_chord(strike);
                } else {
                    self.trigger(strike);
                }
            }
            if matches!(self.explorer.mode, explorer::Mode::Mapping) {
                ui.checkbox(&mut self.pitched, "At played pitch");
            }
            ui.small(self.explorer.kind.label());
        });
    }
    fn pads(&mut self, ui: &mut egui::Ui, rect: egui::Rect, palette: &AppPalette) {
        let document = self.document.clone();
        let regions = &document.instruments[self.instrument].regions;
        let columns = ((rect.width() / 150.0) as usize).max(1);
        let mut selected = None;
        ui.allocate_ui_at_rect(rect, |ui| {
            ui.set_clip_rect(rect.intersect(ui.clip_rect()));
            egui::ScrollArea::vertical()
                .id_source("sampler-pads")
                .show_rows(ui, 72.0, regions.len().div_ceil(columns), |ui, rows| {
                    for row in rows {
                        let width = (ui.available_width()
                            - ui.spacing().item_spacing.x * (columns - 1) as f32)
                            / columns as f32;
                        ui.horizontal(|ui| {
                            for (index, region) in
                                regions.iter().enumerate().skip(row * columns).take(columns)
                            {
                                let label = format!(
                                    "{} · {}–{}\n{}",
                                    surface::note(region.root),
                                    region.velocity[0],
                                    region.velocity[1],
                                    region_label(region).chars().take(32).collect::<String>()
                                );
                                let mut layout = egui::text::LayoutJob::simple(
                                    label,
                                    egui::TextStyle::Button.resolve(ui.style()),
                                    egui::Color32::PLACEHOLDER,
                                    (width - 16.0).max(1.0),
                                );
                                layout.wrap.max_rows = 2;
                                layout.wrap.break_anywhere = true;
                                let label = ui.fonts(|fonts| fonts.layout_job(layout));
                                let (r, g, b) = palette.midi_note_color(0, 0, 1, region.root, 100);
                                if ui
                                    .add_sized(
                                        egui::vec2(width, 64.0),
                                        egui::Button::new(label)
                                            .fill(
                                                egui::Color32::from_rgb(r, g, b)
                                                    .gamma_multiply(0.25),
                                            )
                                            .selected(self.selected == index),
                                    )
                                    .on_hover_text(&region.name)
                                    .clicked()
                                {
                                    selected = Some(index);
                                }
                            }
                        });
                    }
                });
        });
        if let Some(index) = selected {
            self.select(self.instrument, index);
            self.pitched = false;
            self.play();
        }
    }
    fn playback_ui(&mut self, ui: &mut egui::Ui, colors: WorkbenchColors) {
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    self.current().is_some_and(|region| {
                        self.strike
                            .is_none_or(|strike| surface::matches(region, strike))
                    }) && self.task.is_none(),
                    egui::Button::new("Replay sample"),
                )
                .clicked()
            {
                self.play();
            }
            if ui
                .add_enabled(
                    self.player.is_some() || self.task.is_some() || self.midi.active(),
                    egui::Button::new("Stop"),
                )
                .clicked()
            {
                self.stop();
            }
            if self.task.is_some() {
                ui.spinner();
            }
            if let Some(error) = self.midi.error() {
                ui.colored_label(colors.danger, format!("MIDI: {error}"));
            }
            let sounding = self.sounding_keys();
            if !sounding.is_empty() {
                ui.strong(
                    sounding
                        .into_iter()
                        .map(surface::note)
                        .collect::<Vec<_>>()
                        .join(" · "),
                );
            }
            if let Some(audio) = &self.audio {
                ui.label(format!("{:.2} s · effects omitted", audio.seconds));
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
                let x =
                    wave.left() + wave.width() * (index as f32 + 0.5) / audio.peaks.len() as f32;
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
            self.midi = Default::default();
            self.stop();
            self.audio = None;
            self.instrument = instrument;
            self.selected = 0;
            self.chord.clear();
            self.chord_regions.clear();
            self.explorer = explorer::Explorer::new(&self.document.instruments[instrument].regions);
            self.strike = None;
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
                        format!(
                            "{} · {}",
                            surface::note(instrument.regions[index].root),
                            region_label(&instrument.regions[index])
                        ),
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
        self.layers(ui);
        egui::ScrollArea::vertical()
            .id_source("multisample-properties")
            .auto_shrink([false, false])
            .min_scrolled_height(0.0)
            .show(ui, |ui| {
                if let Some(region) = self.current() {
                    region_details(ui, region);
                }
                if !self.document.notes.is_empty() {
                    ui.collapsing("Format notes", |ui| {
                        for note in &self.document.notes {
                            ui.add(egui::Label::new(note).wrap(true));
                        }
                    });
                }
            });
    }
}
fn region_label(region: &Region) -> &str {
    region
        .name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&region.name)
}

fn region_details(ui: &mut egui::Ui, region: &Region) {
    ui.strong(region_label(region));
    ui.label(format!(
        "Keys {}–{} · root {} (MIDI {})",
        surface::note(region.key[0]),
        surface::note(region.key[1]),
        surface::note(region.root),
        region.root
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
    for (key, value) in region.properties.iter().filter(|(key, _)| {
        key.starts_with("sw_")
            || matches!(
                key.as_str(),
                "group" | "Group" | "group.name" | "group_label" | "seq_length" | "seq_position"
            )
    }) {
        ui.label(format!("{key}: {value}"));
    }
    ui.collapsing("Source properties", |ui| {
        for (key, value) in &region.properties {
            ui.strong(key);
            ui.add(egui::Label::new(value).wrap(true));
        }
    });
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.stop();
    }
}
#[cfg(test)]
mod tests;
