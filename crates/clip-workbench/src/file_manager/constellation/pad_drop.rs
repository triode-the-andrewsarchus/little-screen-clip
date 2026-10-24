//! Copy-only pad assignment. Decode first, then atomically install references.
use super::{
    ConstellationState,
    background::{Output, Task},
    sample_pack::{PackSlot, SlotSample},
};
use std::path::PathBuf;
use std::time::Instant;

pub(super) struct PreparedDrop {
    pub(super) index: usize,
    pub(super) before: PackSlot,
    pub(super) append: bool,
    pub(super) samples: Vec<SlotSample>,
}

pub(super) struct DuplicateLayers {
    drop: PreparedDrop,
    token: Option<u64>,
    generation: u64,
}

impl ConstellationState {
    /// Accepts sample references without moving or modifying their source files.
    /// Native completion waits for decoding and a successful, non-stale assignment.
    pub(crate) fn drop_on_pad(
        &mut self,
        index: usize,
        paths: Vec<PathBuf>,
        append: bool,
        token: Option<u64>,
    ) -> bool {
        let Some(before) = self.pack_bank.slots.get(index).cloned() else {
            return false;
        };
        if paths.is_empty()
            || paths.len() + if append { before.samples.len() } else { 0 } > 8
            || self.background.len() >= 16
        {
            self.browser_status =
                Some(("A pad supports up to eight layers".into(), Instant::now()));
            return false;
        }
        let categories: Vec<_> = paths
            .iter()
            .map(|path| {
                self.samples
                    .iter()
                    .find(|sample| sample.file_path == *path)
                    .map(|sample| sample.category)
            })
            .collect();
        self.drop_sequence = self.drop_sequence.wrapping_add(1);
        self.submit_background(
            Task::Drop(self.drop_sequence, self.bank_generation, token),
            move || {
                let mut samples = Vec::with_capacity(paths.len());
                for (path, category) in paths.iter().zip(&categories) {
                    let filename = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    let mut sample = SlotSample::new(path.clone(), filename, *category, 15.0);
                    sample.ensure_audio_loaded();
                    if sample.cached_audio.is_none() {
                        return Err(format!("Could not decode {}", path.display()));
                    }
                    samples.push(sample);
                }
                Ok(Output::Dropped(PreparedDrop {
                    index,
                    before: before.clone(),
                    append,
                    samples,
                }))
            },
        );
        true
    }

    /// Defers duplicate assignments until explicit approval, retaining native completion.
    pub(super) fn accept_pad_layers(&mut self, drop: PreparedDrop, token: Option<u64>) {
        let mut seen: std::collections::HashSet<_> = if drop.append {
            drop.before
                .samples
                .iter()
                .map(|sample| sample.file_path.clone())
                .collect()
        } else {
            Default::default()
        };
        let duplicate = drop
            .samples
            .iter()
            .any(|sample| !seen.insert(sample.file_path.clone()));
        if duplicate {
            if let Some(previous) = self.duplicate_layers.take()
                && let Some(token) = previous.token
            {
                self.native_drop_results.push((token, false));
            }
            self.duplicate_layers = Some(DuplicateLayers {
                drop,
                token,
                generation: self.bank_generation,
            });
        } else {
            let success = self.apply_pad_drop(drop);
            if let Some(token) = token {
                self.native_drop_results.push((token, success));
            }
        }
    }

    pub(crate) fn render_duplicate_warning(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.duplicate_layers else {
            return;
        };
        let mut decision = None;
        egui::Window::new("Duplicate sample layers")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(format!(
                    "Pad {} already contains this sample, or the drop includes it more than once.",
                    pending.drop.index + 1
                ));
                ui.label("Identical layers can increase loudness and reinforce the same sound.");
                ui.horizontal(|ui| {
                    if ui.button("Add anyway").clicked() {
                        decision = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        decision = Some(false);
                    }
                });
            });
        if let Some(approved) = decision {
            let pending = self.duplicate_layers.take().unwrap();
            let success = approved
                && pending.generation == self.bank_generation
                && self.apply_pad_drop(pending.drop);
            if let Some(token) = pending.token {
                self.native_drop_results.push((token, success));
            }
        }
    }

    pub(super) fn apply_pad_drop(&mut self, drop: PreparedDrop) -> bool {
        if self.pack_bank.slots.get(drop.index) != Some(&drop.before) {
            self.browser_status = Some((
                "Pad changed while samples loaded; drop again".into(),
                Instant::now(),
            ));
            return false;
        }
        // Keep the same global PCM ceiling as background pad warming. Dropped
        // layers are already bounded individually, but a whole bank must be too.
        let bytes: usize = self
            .pack_bank
            .slots
            .iter()
            .enumerate()
            .filter(|(index, _)| drop.append || *index != drop.index)
            .flat_map(|(_, slot)| &slot.samples)
            .chain(drop.samples.iter())
            .filter_map(|sample| sample.cached_audio.as_ref())
            .map(|audio| audio.byte_len())
            .sum();
        if bytes > 128 * 1024 * 1024 {
            self.browser_status = Some((
                "PCM memory limit reached; remove layers first".into(),
                Instant::now(),
            ));
            return false;
        }
        self.checkpoint_bank(false);
        self.playback.lock().unwrap().voices[drop.index]
            .drain(..)
            .for_each(|player| player.stop());
        let slot = &mut self.pack_bank.slots[drop.index];
        if !drop.append {
            slot.samples.clear();
        }
        for sample in &drop.samples {
            self.failed_audio.remove(&sample.file_path);
        }
        slot.samples.extend(drop.samples);
        self.selected_slot = drop.index;
        self.checkpoint_bank(false);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_layering_stale_drop_and_undo() {
        let mut state = ConstellationState::default();
        let sample = |name: &str| SlotSample::new(name.into(), name.into(), None, 1.0);
        state.pack_bank.slots[0].samples.push(sample("old"));
        let before = state.pack_bank.slots[0].clone();
        assert!(state.apply_pad_drop(PreparedDrop {
            index: 0,
            before: before.clone(),
            append: false,
            samples: vec![sample("new")]
        }));
        assert_eq!(state.pack_bank.slots[0].samples[0].filename, "new");
        assert!(!state.apply_pad_drop(PreparedDrop {
            index: 0,
            before,
            append: false,
            samples: vec![sample("stale")]
        }));
        assert!(state.apply_pad_drop(PreparedDrop {
            index: 0,
            before: state.pack_bank.slots[0].clone(),
            append: true,
            samples: vec![sample("layer")]
        }));
        assert_eq!(state.pack_bank.slots[0].samples.len(), 2);
        state.undo_bank(false);
        assert_eq!(state.pack_bank.slots[0].samples.len(), 1);
        assert_eq!(state.pack_bank.slots[0].samples[0].filename, "new");
    }
}
