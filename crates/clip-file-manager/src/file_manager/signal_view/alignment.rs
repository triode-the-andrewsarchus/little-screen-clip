//! Clock calibration from matched landmarks and waveform correlation.
mod landmarks;
mod measure;
mod waveform;

use super::{Source, inputs::Input, timeline::LandmarkMarker};
use crate::{
    file_manager::capture_browse::{Job, keyed_poll, start},
    file_source::FileSource,
};
use egui::{Context, Ui};
use landmarks::Landmarks;
pub(super) use landmarks::Settings;
use little_clip_core::{
    BackgroundJobs,
    engineering::{analysis::Clock, waveforms::display_value},
};
use serde::{Deserialize, Serialize};
use std::{ops::Range, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::file_manager::signal_view) struct PairKey {
    reference: u64,
    target: u64,
}

impl PairKey {
    fn slot(self, source: u64) -> Option<usize> {
        [self.reference, self.target]
            .iter()
            .position(|&id| id == source)
    }

    fn sources(self, sources: &[Source]) -> Option<[&Source; 2]> {
        let find = |id| {
            sources
                .iter()
                .find(|source| source.id == id && source.capture.file().is_some())
        };
        if self.reference == self.target {
            return None;
        }
        Some([find(self.reference)?, find(self.target)?])
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Suggestion {
    pub recordings: PairKey,
    pub reference_clock: Clock,
    pub base: Clock,
    pub clock: Clock,
}

impl Suggestion {
    pub(super) fn target(self) -> u64 {
        self.recordings.target
    }

    fn belongs_to(self, sources: &[Source]) -> bool {
        self.recordings
            .sources(sources)
            .is_some_and(|[reference, target]| {
                reference.clock == self.reference_clock
                    && target.clock == self.base
                    && target.accepts_clock(self.clock)
            })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Method {
    #[default]
    Landmarks,
    Waveform,
}

#[derive(Default)]
pub(super) struct Alignment {
    pub open: bool,
    method: Method,
    recordings: Option<PairKey>,
    marks: Landmarks,
    waveform: waveform::Waveform,
    previewing: bool,
    checking: Option<(Suggestion, Job<()>)>,
    error: Option<String>,
}

pub(super) enum Action {
    Apply(Suggestion),
    Reveal(Visit),
}

pub(super) struct Visit {
    pub source: u64,
    pub endpoints: [(u64, f64); 2],
}

impl Alignment {
    pub(super) fn toggle(&mut self, visible: &Range<f64>, reference: Option<f64>) {
        self.open = !self.open;
        if self.open {
            self.waveform.use_region(visible, reference);
        } else {
            self.clear_proposal();
        }
    }

    pub(super) fn ui(
        &mut self,
        ui: &mut Ui,
        sources: &[Source],
        visible: &Range<f64>,
        pinned: Option<f64>,
    ) -> Option<Action> {
        if !self.open {
            return None;
        }
        let approved = self.poll_approval(sources);
        ui.separator();
        match self.recording_controls(ui, sources) {
            Some(recordings) => {
                let before = self.method;
                ui.horizontal_wrapped(|ui| {
                    for (label, method) in [
                        ("Landmark pairs", Method::Landmarks),
                        ("Waveform match", Method::Waveform),
                    ] {
                        if ui
                            .add(egui::Button::new(label).selected(self.method == method))
                            .clicked()
                        {
                            self.method = method;
                        }
                    }
                });
                if before != self.method {
                    self.clear_proposal();
                }
                let goto = self.method_ui(ui, sources, recordings, visible, pinned);
                let proposal = self.suggestion(sources);
                self.correction_controls(ui, sources, proposal);
                ui.separator();
                approved.map(Action::Apply).or(goto.map(Action::Reveal))
            }
            None => {
                ui.weak("Clock alignment needs two available recordings.");
                None
            }
        }
    }

    fn recording_controls(&mut self, ui: &mut Ui, sources: &[Source]) -> Option<PairKey> {
        let mut available = sources
            .iter()
            .filter(|source| source.capture.file().is_some());
        let mut selected = self
            .recordings
            .filter(|pair| pair.sources(sources).is_some())
            .or_else(|| {
                Some(PairKey {
                    reference: available.next()?.id,
                    target: available.next()?.id,
                })
            })?;
        let current = selected;
        ui.horizontal_wrapped(|ui| {
            for (label, slot, other) in [
                ("Reference", &mut selected.reference, current.target),
                ("Target", &mut selected.target, current.reference),
            ] {
                ui.strong(label);
                for source in sources
                    .iter()
                    .filter(|source| source.capture.file().is_some() && source.id != other)
                {
                    if ui
                        .add(egui::Button::new(source.name()).selected(*slot == source.id))
                        .on_hover_text(source.path.to_string_lossy())
                        .clicked()
                    {
                        *slot = source.id;
                    }
                }
                ui.add_space(6.0);
            }
            if ui.button("Swap roles").clicked() {
                std::mem::swap(&mut selected.reference, &mut selected.target);
            }
        });
        if selected.reference == selected.target {
            selected = current;
        }
        if self.recordings != Some(selected) {
            self.recordings = Some(selected);
            self.marks.reset_cursor_pick();
            self.clear_proposal();
        }
        Some(selected)
    }

    fn method_ui(
        &mut self,
        ui: &mut Ui,
        sources: &[Source],
        recordings: PairKey,
        visible: &Range<f64>,
        pinned: Option<f64>,
    ) -> Option<Visit> {
        match self.method {
            Method::Landmarks => self.marks.ui(ui, recordings, sources),
            Method::Waveform => {
                let entries: Vec<_> =
                    sources
                        .iter()
                        .filter(|source| recordings.slot(source.id).is_some())
                        .flat_map(|source| {
                            source.inputs().into_iter().filter(readable).map(|input| {
                                waveform::Entry {
                                    source: source.id,
                                    input,
                                }
                            })
                        })
                        .collect();
                self.waveform
                    .ui(ui, &entries, recordings, visible, pinned, self.previewing);
                None
            }
        }
    }

    fn correction_controls(
        &mut self,
        ui: &mut Ui,
        sources: &[Source],
        proposal: Option<Suggestion>,
    ) {
        let changed = proposal.is_some_and(|proposal| proposal.base != proposal.clock);
        ui.horizontal_wrapped(|ui| {
            if let Some(proposal) = proposal {
                let clock = proposal.clock;
                ui.monospace(format!(
                    "Epoch {}   Offset {}   {:+.3} ppm",
                    display_value(clock.native_epoch(), "s"),
                    display_value(clock.epoch_offset(), "s"),
                    (clock.scale() - 1.0) * 1e6
                ))
                .on_hover_text(
                    "Target native epoch, displacement at that epoch, and change in elapsed time",
                );
            }
            if ui
                .add_enabled(
                    changed,
                    egui::Button::new("Preview correction").selected(self.previewing && changed),
                )
                .clicked()
            {
                self.previewing = !self.previewing;
            }
            if ui
                .add_enabled(
                    changed && self.checking.is_none(),
                    egui::Button::new("Apply correction"),
                )
                .clicked()
                && let Some(proposal) = proposal
            {
                self.check_approval(ui.ctx(), sources, proposal);
            }
            if proposal.is_some() && !changed {
                ui.weak("Applied");
            }
            if self.checking.is_some() {
                ui.spinner();
                ui.ctx().request_repaint_after(Duration::from_millis(24));
            }
        });
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }

    fn suggestion(&self, sources: &[Source]) -> Option<Suggestion> {
        let proposal = match self.method {
            Method::Landmarks => self.marks.suggestion(self.recordings?, sources),
            Method::Waveform => self.waveform.suggestion(),
        }?;
        proposal.belongs_to(sources).then_some(proposal)
    }

    pub(super) fn preview(&self, sources: &[Source]) -> Option<Suggestion> {
        (self.open && self.previewing)
            .then(|| self.suggestion(sources))
            .flatten()
    }

    pub(super) fn cursor_button(&mut self, ui: &mut Ui, source: u64, cursor: Option<f64>) {
        if self.open
            && self.method == Method::Landmarks
            && let Some(pair) = self.recordings
            && let Some(slot) = pair.slot(source)
        {
            self.marks.cursor_button(ui, pair, slot, cursor);
        }
    }

    pub(super) fn marker(&self, source: u64) -> Option<LandmarkMarker> {
        if self.open && self.method == Method::Landmarks {
            self.marks.marker(self.recordings?, source)
        } else {
            None
        }
    }

    fn clear_proposal(&mut self) {
        self.previewing = false;
        self.checking = None;
        self.waveform.retire();
        self.error = None;
    }

    pub(super) fn forget(&mut self, source: u64) {
        self.marks.forget(source);
        if self
            .recordings
            .is_some_and(|pair| pair.slot(source).is_some())
        {
            self.recordings = None;
            self.clear_proposal();
        }
    }

    fn check_approval(&mut self, ctx: &Context, sources: &[Source], proposal: Suggestion) {
        let Some(files) = files(proposal.recordings, sources) else {
            return;
        };
        self.error = None;
        self.checking = Some((
            proposal,
            start(
                BackgroundJobs::global(),
                ctx,
                Duration::from_secs(20),
                move |_| verify_recordings(&files),
            ),
        ));
    }

    fn poll_approval(&mut self, sources: &[Source]) -> Option<Suggestion> {
        let (proposal, verified) = keyed_poll(&mut self.checking)?;
        if self.suggestion(sources) != Some(proposal) {
            return None;
        }
        match verified {
            Ok(()) => {
                self.previewing = false;
                Some(proposal)
            }
            Err(error) => {
                self.previewing = false;
                self.error = Some(error);
                None
            }
        }
    }

    pub(super) fn save(&self) -> Settings {
        self.marks.save(self.recordings)
    }

    pub(super) fn analysis_edits(&self) -> Settings {
        self.marks.save(None)
    }

    pub(super) fn restore(&mut self, saved: Settings, sources: &[Source]) -> Result<(), String> {
        self.recordings = self.marks.restore(saved, sources)?;
        Ok(())
    }
}

fn readable(input: &Input) -> bool {
    !input.is_awaiting_filter() && !input.unavailable()
}

fn files(recordings: PairKey, sources: &[Source]) -> Option<[FileSource; 2]> {
    let [reference, target] = recordings.sources(sources)?;
    Some([
        reference.capture.file()?.clone(),
        target.capture.file()?.clone(),
    ])
}

fn verify_recordings(files: &[FileSource; 2]) -> Result<(), String> {
    for source in files {
        source.check().map_err(|error| {
            format!(
                "Clock correction needs the original recording {}. {error}",
                source
                    .path()
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
            )
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changing_a_recording_rejects_pending_calibration() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.vcd");
        let second = dir.path().join("second.vcd");
        std::fs::write(&first, "reference").unwrap();
        std::fs::write(&second, "target").unwrap();
        let files = [
            FileSource::capture(&first).unwrap(),
            FileSource::capture(&second).unwrap(),
        ];
        verify_recordings(&files).unwrap();
        std::fs::write(second, "changed target").unwrap();
        assert!(
            verify_recordings(&files)
                .unwrap_err()
                .contains("second.vcd")
        );
    }
}
