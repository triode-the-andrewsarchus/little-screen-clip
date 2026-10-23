//! Bounded document history and unsaved-work admission, separate from audio caches.
use super::{ConstellationState, SamplePackBank};
use std::path::{Path, PathBuf};

pub(super) fn metadata(bank: &SamplePackBank) -> SamplePackBank {
    let mut snapshot = bank.clone();
    for sample in snapshot.slots.iter_mut().flat_map(|slot| &mut slot.samples) {
        sample.cached_audio = None;
    }
    snapshot
}

pub(super) struct BankHistory {
    observed: SamplePackBank,
    saved: SamplePackBank,
    undo: Vec<SamplePackBank>,
    redo: Vec<SamplePackBank>,
    coalescing: bool,
}

impl BankHistory {
    pub fn new(bank: &SamplePackBank) -> Self {
        Self {
            observed: metadata(bank),
            saved: metadata(bank),
            undo: vec![],
            redo: vec![],
            coalescing: false,
        }
    }

    pub fn dirty(&self, bank: &SamplePackBank) -> bool {
        *bank != self.saved
    }

    pub fn checkpoint(&mut self, bank: &SamplePackBank, coalesce: bool) {
        let changed = *bank != self.observed;
        if changed {
            if !self.coalescing {
                if self.undo.len() == 32 {
                    self.undo.remove(0);
                }
                self.undo.push(self.observed.clone());
            }
            self.redo.clear();
            self.observed = metadata(bank);
        }
        self.coalescing = coalesce && (changed || self.coalescing);
    }

    pub fn saved(&mut self, snapshot: SamplePackBank) {
        self.saved = metadata(&snapshot);
    }

    pub fn prepared(&mut self, bank: &SamplePackBank, was_clean: bool) {
        self.observed = metadata(bank);
        if was_clean {
            self.saved = self.observed.clone();
        }
    }

    fn step(&mut self, bank: &SamplePackBank, redo: bool) -> Option<SamplePackBank> {
        self.checkpoint(bank, false);
        let (from, to) = if redo {
            (&mut self.redo, &mut self.undo)
        } else {
            (&mut self.undo, &mut self.redo)
        };
        let next = from.pop()?;
        to.push(metadata(bank));
        self.observed = next.clone();
        Some(next)
    }
}

pub(super) enum BankAction {
    Load(PathBuf),
    Close,
}

impl ConstellationState {
    pub(super) fn checkpoint_bank(&mut self, coalesce: bool) {
        self.history.checkpoint(&self.pack_bank, coalesce);
    }

    pub fn bank_dirty(&self) -> bool {
        self.history.dirty(&self.pack_bank)
    }

    pub(super) fn undo_bank(&mut self, redo: bool) {
        if let Some(bank) = self.history.step(&self.pack_bank, redo) {
            self.stop_playback();
            self.bank_generation = self.bank_generation.wrapping_add(1);
            self.pack_bank = bank;
            self.failed_audio.clear();
        }
    }

    /// Defers close while unsaved edits need a decision. Accepted saves remain owned.
    pub fn allow_close(&mut self) -> bool {
        if std::mem::take(&mut self.close_approved) {
            return true;
        }
        if !self.bank_dirty() {
            return true;
        }
        self.pending_bank_action = Some(BankAction::Close);
        false
    }

    pub(super) fn continue_bank_action(&mut self) {
        match self.pending_bank_action.take() {
            Some(BankAction::Load(path)) => {
                self.load_approved = true;
                self.load_browser_bank(path, true);
            }
            Some(BankAction::Close) => self.close_approved = true,
            None => {}
        }
    }

    /// Shows an inline decision bar even when another media view is active.
    pub fn render_bank_guard(&mut self, ctx: &egui::Context, folder: &Path) {
        if self.pending_bank_action.is_none() {
            return;
        }
        egui::TopBottomPanel::top("unsaved_sample_pack").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.strong("This sample pack has unsaved changes.");
                if ui
                    .add_enabled(!self.writing_pack(), egui::Button::new("Save and continue"))
                    .clicked()
                {
                    let root = self
                        .current_indexed_folder
                        .as_deref()
                        .unwrap_or(folder)
                        .to_path_buf();
                    self.save_bank(root, self.pack_bank.clone());
                    self.guard_saving = self.writing_pack();
                }
                if ui
                    .add_enabled(!self.writing_pack(), egui::Button::new("Discard changes"))
                    .clicked()
                {
                    self.continue_bank_action();
                }
                if ui.button("Keep editing").clicked() {
                    self.pending_bank_action = None;
                    self.guard_saving = false;
                }
            });
        });
    }

    pub(super) fn render_history_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.weak(if self.bank_dirty() {
                "Unsaved changes"
            } else {
                "Saved"
            });
            if ui
                .add_enabled(!self.history.undo.is_empty(), egui::Button::new("Undo"))
                .clicked()
            {
                self.undo_bank(false);
            }
            if ui
                .add_enabled(!self.history.redo.is_empty(), egui::Button::new("Redo"))
                .clicked()
            {
                self.undo_bank(true);
            }
            ui.add_enabled_ui(!self.pack_bank.slots[self.selected_slot].is_empty(), |ui| {
                ui.toggle_value(&mut self.pad_editor_open, "Editor");
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_coalesce_redo_branches_and_save_completion_preserves_later_edits() {
        let mut bank = SamplePackBank::default();
        let mut history = BankHistory::new(&bank);
        bank.name = "A".into();
        history.checkpoint(&bank, true);
        bank.name = "AB".into();
        history.checkpoint(&bank, true);
        history.checkpoint(&bank, false);
        let saved = metadata(&bank);
        bank.name = "ABC".into();
        history.checkpoint(&bank, false);
        history.saved(saved);
        assert!(history.dirty(&bank));
        bank = history.step(&bank, false).unwrap();
        assert_eq!(bank.name, "AB");
        assert!(!history.dirty(&bank));
        bank = history.step(&bank, false).unwrap();
        assert_eq!(bank.name, "Untitled Pack");
        bank = history.step(&bank, true).unwrap();
        assert_eq!(bank.name, "AB");
        bank.name = "Other".into();
        history.checkpoint(&bank, false);
        assert!(history.step(&bank, true).is_none());
    }

    #[test]
    fn empty_gestures_do_not_swallow_the_first_edit_and_history_is_bounded() {
        let mut bank = SamplePackBank::default();
        let mut history = BankHistory::new(&bank);
        history.checkpoint(&bank, true);
        bank.name = "First".into();
        history.checkpoint(&bank, true);
        assert_eq!(history.undo.len(), 1);
        history.checkpoint(&bank, false);
        for index in 0..100 {
            bank.name = index.to_string();
            history.checkpoint(&bank, false);
        }
        assert_eq!(history.undo.len(), 32);
        assert!(
            history
                .undo
                .iter()
                .flat_map(|bank| &bank.slots)
                .flat_map(|slot| &slot.samples)
                .all(|sample| sample.cached_audio.is_none())
        );
    }

    #[test]
    fn load_protection_keeps_the_current_bank_and_only_approves_one_attempt() {
        let mut state = ConstellationState::new();
        state.pack_bank.name = "Keep me".into();
        state.load_browser_bank("/other".into(), true);
        assert!(matches!(
            state.pending_bank_action,
            Some(BankAction::Load(_))
        ));
        assert_eq!(state.pack_bank.name, "Keep me");
        assert!(state.pending_load_bank.is_none());
        // A successful Save-and-continue must consume approval even while clean.
        state.history.saved(metadata(&state.pack_bank));
        state.continue_bank_action();
        assert!(!state.load_approved);
    }

    #[test]
    fn close_requires_discard_or_a_successful_save() {
        let mut state = ConstellationState::new();
        state.pack_bank.name = "Unsaved".into();
        assert!(!state.allow_close());
        state.continue_bank_action(); // Explicit discard approval.
        assert!(state.allow_close());
        assert!(!state.allow_close());
    }
}
