use super::{PairKey, Source, Suggestion, Visit};
use crate::file_manager::signal_view::timeline::LandmarkMarker;
use egui::Ui;
use little_clip_core::engineering::{
    analysis::{Clock, Landmark, fit_clock},
    waveforms::display_value,
};
use serde::{Deserialize, Serialize};

const MAX_PAIRS: usize = 64;

#[derive(Clone, Copy, Deserialize, Serialize)]
struct PairTimes {
    native: [f64; 2],
    included: bool,
}

impl PairTimes {
    fn endpoints(self, recordings: PairKey) -> [(u64, f64); 2] {
        [
            (recordings.reference, self.native[0]),
            (recordings.target, self.native[1]),
        ]
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct RecordingPairs {
    recordings: PairKey,
    pairs: Vec<PairTimes>,
}

#[derive(Default, Deserialize, Serialize)]
pub(in crate::file_manager::signal_view) struct Settings {
    recordings: Option<PairKey>,
    sets: Vec<RecordingPairs>,
}

#[derive(Default)]
pub(super) struct Landmarks {
    sets: Vec<RecordingPairs>,
    pending: [Option<f64>; 2],
    selected: Option<usize>,
}

impl Landmarks {
    fn pairs(&self, recordings: PairKey) -> &[PairTimes] {
        self.sets
            .iter()
            .find(|set| set.recordings == recordings)
            .map_or(&[], |set| &set.pairs)
    }

    fn pairs_mut(&mut self, recordings: PairKey) -> &mut Vec<PairTimes> {
        let index = self
            .sets
            .iter()
            .position(|set| set.recordings == recordings)
            .unwrap_or_else(|| {
                self.sets.push(RecordingPairs {
                    recordings,
                    pairs: Vec::new(),
                });
                self.sets.len() - 1
            });
        &mut self.sets[index].pairs
    }

    fn fit(
        &self,
        recordings: PairKey,
        reference: &Source,
        target: &Source,
    ) -> Result<Clock, &'static str> {
        let ranges = [reference.capture.domain(), target.capture.domain()];
        let pairs = self.pairs(recordings).iter().filter(|pair| pair.included);
        if pairs.clone().any(|pair| {
            pair.native
                .iter()
                .zip(&ranges)
                .any(|(time, range)| !range.contains(time))
        }) {
            return Err("An included landmark is outside its recording.");
        }
        let landmarks = pairs
            .map(|pair| Landmark {
                native: pair.native[1],
                workspace: reference.clock.to_workspace(pair.native[0]),
            })
            .collect::<Vec<_>>();
        fit_clock(&landmarks, target.clock.scale())
    }

    pub(super) fn suggestion(&self, recordings: PairKey, sources: &[Source]) -> Option<Suggestion> {
        let [reference, target] = recordings.sources(sources)?;
        Some(Suggestion {
            recordings,
            base: target.clock,
            reference_clock: reference.clock,
            clock: self.fit(recordings, reference, target).ok()?,
        })
    }

    pub(super) fn cursor_button(
        &mut self,
        ui: &mut Ui,
        recordings: PairKey,
        slot: usize,
        cursor: Option<f64>,
    ) {
        let available = self.pairs(recordings).len() < MAX_PAIRS && cursor.is_some();
        if ui
            .add_enabled(available, egui::Button::new("Take landmark"))
            .on_disabled_hover_text(
                "Place a cursor on this recording, or remove a pair if the table is full.",
            )
            .clicked()
        {
            self.pending[slot] = cursor;
            if let [Some(reference), Some(target)] = self.pending {
                let pairs = self.pairs_mut(recordings);
                pairs.push(PairTimes {
                    native: [reference, target],
                    included: true,
                });
                self.selected = Some(pairs.len() - 1);
                self.pending = [None, None];
            }
        }
        if let Some(time) = self.pending[slot] {
            ui.monospace(display_value(time, "s"));
            if ui.small_button("Clear pick").clicked() {
                self.pending[slot] = None;
            }
        }
    }

    pub(super) fn ui(
        &mut self,
        ui: &mut Ui,
        recordings: PairKey,
        sources: &[Source],
    ) -> Option<Visit> {
        let [reference, target] = recordings.sources(sources)?;
        if self.pairs(recordings).is_empty() {
            ui.weak("Place a cursor on each recording and use Take landmark.");
            return None;
        }
        let fit = self.fit(recordings, reference, target);
        if let Err(error) = fit {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if let Ok(clock) = fit
            && !target.accepts_clock(clock)
        {
            ui.colored_label(
                ui.visuals().error_fg_color,
                "This correction exceeds the timeline precision.",
            );
        }
        let selected = self.selected;
        let table = ui
            .push_id((recordings.reference, recordings.target), |ui| {
                PairTable::show(
                    ui,
                    self.pairs_mut(recordings),
                    selected,
                    recordings,
                    reference.clock,
                    fit.ok(),
                )
            })
            .inner;
        if let Some((row, _)) = &table.visit {
            let row = *row;
            self.selected = Some(row);
        }
        if let Some(row) = table.removed {
            self.selected = selected.and_then(|selected| match selected.cmp(&row) {
                std::cmp::Ordering::Less => Some(selected),
                std::cmp::Ordering::Equal => None,
                std::cmp::Ordering::Greater => Some(selected - 1),
            });
        }
        if let Some(row) = self.selected
            && let Some(pair) = self.pairs_mut(recordings).get_mut(row)
        {
            edit_pair(
                ui,
                row,
                pair,
                [reference.capture.domain(), target.capture.domain()],
            );
        }
        table.visit.map(|(_, visit)| visit)
    }

    pub(super) fn marker(&self, recordings: PairKey, source: u64) -> Option<LandmarkMarker> {
        let slot = recordings.slot(source)?;
        let time = self.pending[slot].or_else(|| {
            self.pairs(recordings)
                .get(self.selected?)
                .map(|pair| pair.native[slot])
        })?;
        Some(LandmarkMarker {
            time,
            reference: slot == 0,
        })
    }

    pub(super) fn reset_cursor_pick(&mut self) {
        self.pending = [None, None];
        self.selected = None;
    }

    pub(super) fn forget(&mut self, source: u64) {
        self.sets
            .retain(|set| set.recordings.slot(source).is_none());
        self.reset_cursor_pick();
    }

    pub(super) fn save(&self, recordings: Option<PairKey>) -> Settings {
        Settings {
            recordings,
            sets: self.sets.clone(),
        }
    }

    pub(super) fn restore(
        &mut self,
        settings: Settings,
        sources: &[Source],
    ) -> Result<Option<PairKey>, String> {
        let knows = |id| sources.iter().any(|source| source.id == id);
        let valid = |pair: PairKey| {
            pair.reference != pair.target && knows(pair.reference) && knows(pair.target)
        };
        if settings.recordings.is_some_and(|pair| !valid(pair)) {
            return Err("Invalid saved clock comparison".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        for set in &settings.sets {
            if !valid(set.recordings)
                || !seen.insert((set.recordings.reference, set.recordings.target))
                || set.pairs.len() > MAX_PAIRS
                || set
                    .pairs
                    .iter()
                    .flat_map(|pair| pair.native)
                    .any(|time| !time.is_finite())
            {
                return Err("Invalid saved landmarks".into());
            }
        }
        self.sets = settings.sets;
        self.reset_cursor_pick();
        Ok(settings.recordings)
    }
}

fn edit_pair(ui: &mut Ui, row: usize, pair: &mut PairTimes, domains: [std::ops::Range<f64>; 2]) {
    ui.push_id(("landmark times", row), |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("Pair {}", row + 1));
            for ((label, time), domain) in ["Reference", "Target"]
                .into_iter()
                .zip(&mut pair.native)
                .zip(domains)
            {
                ui.label(label);
                let speed = (domain.end - domain.start) * 0.0001;
                ui.add(
                    egui::DragValue::new(time)
                        .range(domain.start..=domain.end.next_down())
                        .clamp_existing_to_range(false)
                        .speed(speed)
                        .max_decimals(15)
                        .suffix(" s")
                        .update_while_editing(false),
                )
                .on_hover_text("Time on the original recording clock");
            }
        });
    });
}

#[derive(Default)]
struct PairTable {
    visit: Option<(usize, Visit)>,
    removed: Option<usize>,
}

impl PairTable {
    fn show(
        ui: &mut Ui,
        pairs: &mut Vec<PairTimes>,
        selected: Option<usize>,
        recordings: PairKey,
        reference: Clock,
        fit: Option<Clock>,
    ) -> Self {
        let mut result = Self::default();
        egui::ScrollArea::vertical()
            .id_salt("landmark-pairs")
            .max_height(128.0)
            .show(ui, |ui| {
                egui::Grid::new("pairs")
                    .striped(true)
                    .min_col_width(35.0)
                    .show(ui, |ui| {
                        for label in ["Use", "Reference time", "Target time", "Residual", ""] {
                            ui.strong(label);
                        }
                        ui.end_row();
                        for (row, pair) in pairs.iter_mut().enumerate() {
                            ui.checkbox(&mut pair.included, "")
                                .on_hover_text("Include this pair in the correction");
                            result.endpoints(ui, row, pair, selected, recordings);
                            let residual = fit.map(|clock| {
                                residual_at(
                                    reference.to_workspace(pair.native[0]),
                                    clock.to_workspace(pair.native[1]),
                                )
                            });
                            let residual = residual.map_or_else(
                                || "—".into(),
                                |seconds| {
                                    format!(
                                        "{}{}",
                                        if seconds > 0.0 { "+" } else { "" },
                                        display_value(seconds, "s")
                                    )
                                },
                            );
                            if ui.add(egui::Button::new(egui::RichText::new(residual).monospace()).small())
                                .on_hover_text("Corrected target time minus reference time · reveal both landmarks")
                                .clicked()
                            {
                                result.visit = Some((row, Visit {
                                    source: recordings.target,
                                    endpoints: pair.endpoints(recordings),
                                }));
                            }
                            if ui.small_button("Remove").clicked() {
                                result.removed = Some(row);
                            }
                            ui.end_row();
                        }
                    });
            });
        if let Some(row) = result.removed {
            pairs.remove(row);
        }
        result
    }

    fn endpoints(
        &mut self,
        ui: &mut Ui,
        row: usize,
        pair: &PairTimes,
        selected: Option<usize>,
        recordings: PairKey,
    ) {
        let endpoints = pair.endpoints(recordings);
        for (source, time) in endpoints {
            if ui
                .selectable_label(selected == Some(row), display_value(time, "s"))
                .on_hover_text(format!(
                    "Reveal this landmark · {time:.15} s in the recording"
                ))
                .clicked()
            {
                self.visit = Some((row, Visit { source, endpoints }));
            }
        }
    }
}

fn residual_at(reference: f64, target: f64) -> f64 {
    let difference = target - reference;
    let magnitude = reference.abs().max(target.abs());
    if difference.abs() <= 2.0 * (magnitude.next_up() - magnitude) {
        0.0
    } else {
        difference
    }
}
