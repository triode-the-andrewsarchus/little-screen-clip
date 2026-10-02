use super::*;

impl AudioPreviewState {
    const MIN_GAIN_DB: f32 = -60.0;
    const MAX_GAIN_DB: f32 = 40.0; // 100x
    pub(super) const NORMALIZE_TARGET_DBFS: f32 = -1.0;
    const MAX_UNDO_STEPS: usize = 50;
    const SCROLL_UNDO_WINDOW: Duration = Duration::from_millis(300);

    pub(super) fn edit_snapshot(&self) -> AudioEditSnapshot {
        AudioEditSnapshot {
            path: self.path.clone(),
            duration_sec: self.duration_sec,
            track_gain_db: self.track_gain_db,
            selection_mode: self.selection_mode,
            selection: self.selection,
            multi_selections: self.multi_selections.clone(),
            active_multi_index: self.active_multi_index,
            region_gain_db: self.region_gain_db.clone(),
            single_region_gain_db: self.single_region_gain_db,
            region_crossfade_sec: self.region_crossfade_sec.clone(),
            single_crossfade_sec: self.single_crossfade_sec,
            single_low_pass_hz: self.single_low_pass_hz,
            single_high_pass_hz: self.single_high_pass_hz,
            region_low_pass_hz: self.region_low_pass_hz.clone(),
            region_high_pass_hz: self.region_high_pass_hz.clone(),
        }
    }

    pub(super) fn record_edit(&mut self, before: AudioEditSnapshot) {
        if before == self.edit_snapshot() {
            return;
        }
        if self.undo_stack.len() == Self::MAX_UNDO_STEPS {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(before);
        self.redo_stack.clear();
        self.last_scroll_edit = None;
    }

    pub(super) fn record_scroll_edit(
        &mut self,
        before: AudioEditSnapshot,
        kind: ScrollEditKind,
        now: Instant,
    ) {
        if before == self.edit_snapshot() {
            return;
        }
        let target = match self.selection_mode {
            SelectionMode::Single => self.selection.map(|range| (None, range)),
            SelectionMode::Multi => self.active_multi_index.and_then(|index| {
                self.multi_selections
                    .get(index)
                    .copied()
                    .map(|range| (Some(index), range))
            }),
        };
        let Some((index, range)) = target else {
            self.record_edit(before);
            return;
        };
        let key = ScrollEditKey {
            kind,
            mode: self.selection_mode,
            index,
            range,
        };
        let continues = self.last_scroll_edit.is_some_and(|(previous, at)| {
            previous == key
                && now
                    .checked_duration_since(at)
                    .is_some_and(|elapsed| elapsed <= Self::SCROLL_UNDO_WINDOW)
        });
        if !continues {
            self.record_edit(before);
        }
        self.last_scroll_edit = Some((key, now));
    }

    pub(super) fn restore_edit(&mut self, snapshot: AudioEditSnapshot) {
        let session_changed = self.path != snapshot.path;
        if session_changed {
            self.stop();
        }
        self.path = snapshot.path;
        self.duration_sec = snapshot.duration_sec;
        self.track_gain_db = snapshot.track_gain_db;
        self.selection_mode = snapshot.selection_mode;
        self.selection = snapshot.selection;
        self.multi_selections = snapshot.multi_selections;
        self.active_multi_index = snapshot.active_multi_index;
        self.region_gain_db = snapshot.region_gain_db;
        self.single_region_gain_db = snapshot.single_region_gain_db;
        self.region_crossfade_sec = snapshot.region_crossfade_sec;
        self.single_crossfade_sec = snapshot.single_crossfade_sec;
        self.single_low_pass_hz = snapshot.single_low_pass_hz;
        self.single_high_pass_hz = snapshot.single_high_pass_hz;
        self.region_low_pass_hz = snapshot.region_low_pass_hz;
        self.region_high_pass_hz = snapshot.region_high_pass_hz;
        if session_changed {
            self.current_time_sec = self.current_time_sec.min(self.duration_sec);
            self.view_start_sec = 0.0;
            self.view_duration_sec = self.duration_sec;
            self.restart_session_analysis();
        }
    }

    pub(super) fn undo(&mut self) {
        self.last_scroll_edit = None;
        if let Some(previous) = self.undo_stack.pop() {
            self.redo_stack.push(self.edit_snapshot());
            self.restore_edit(previous);
        }
    }

    pub(super) fn redo(&mut self) {
        self.last_scroll_edit = None;
        if let Some(next) = self.redo_stack.pop() {
            if self.undo_stack.len() == Self::MAX_UNDO_STEPS {
                self.undo_stack.remove(0);
            }
            self.undo_stack.push(self.edit_snapshot());
            self.restore_edit(next);
        }
    }

    pub(super) fn handle_undo_shortcuts(&mut self, ctx: &egui::Context) {
        match ShortcutDispatcher::check_common(ctx) {
            Some(CommonShortcut::Undo) => {
                self.undo();
                ctx.request_repaint();
            }
            Some(CommonShortcut::Redo) => {
                self.redo();
                ctx.request_repaint();
            }
            _ => {}
        }
    }

    pub(super) fn step_gain_db(current: f32, delta_db: f32) -> f32 {
        if delta_db == 0.0 {
            current
        } else if current == f32::NEG_INFINITY {
            if delta_db > 0.0 {
                Self::MIN_GAIN_DB
            } else {
                current
            }
        } else {
            let next = (current + delta_db).min(Self::MAX_GAIN_DB);
            if next < Self::MIN_GAIN_DB {
                f32::NEG_INFINITY
            } else {
                next
            }
        }
    }

    pub fn adjust_active_region_gain(&mut self, delta_db: f32, now: Instant) {
        let before = self.edit_snapshot();
        match self.selection_mode {
            SelectionMode::Single if self.selection.is_some() => {
                self.single_region_gain_db =
                    Self::step_gain_db(self.single_region_gain_db, delta_db);
            }
            SelectionMode::Multi => {
                if let Some(index) = self.active_multi_index {
                    if self.region_gain_db.len() < self.multi_selections.len() {
                        self.region_gain_db.resize(self.multi_selections.len(), 0.0);
                    }
                    if let Some(gain) = self.region_gain_db.get_mut(index) {
                        *gain = Self::step_gain_db(*gain, delta_db);
                    }
                }
            }
            _ => {}
        }
        self.record_scroll_edit(before, ScrollEditKind::Gain, now);
    }

    pub(super) fn peak_amplitude_in_range(&self, range: (f32, f32)) -> Option<f32> {
        let analysis = self.analysis.as_ref()?;
        let (start, end) = AudioSelectionModel::clamp_to_duration(
            AudioSelectionModel::normalize(range),
            self.duration_sec,
        );
        let sample_rate = analysis.sample_rate as f32;
        let first = ((start * sample_rate).floor() as usize).min(analysis.total_frames);
        let last = ((end * sample_rate).ceil() as usize).min(analysis.total_frames);
        if first >= last {
            return None;
        }
        let mut peak = analysis.ch1_samples[first..last]
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        if let Some(ch2) = &analysis.ch2_samples {
            peak = ch2[first..last]
                .iter()
                .fold(peak, |peak, sample| peak.max(sample.abs()));
        }
        (peak.is_finite() && peak > f32::EPSILON).then_some(peak)
    }

    pub(super) fn normalization_gain_db(peak: f32, preceding_gain_db: f32) -> Option<f32> {
        if !peak.is_finite() || peak <= f32::EPSILON || !preceding_gain_db.is_finite() {
            return None;
        }
        Some(
            (Self::NORMALIZE_TARGET_DBFS - 20.0 * peak.log10() - preceding_gain_db)
                .clamp(Self::MIN_GAIN_DB, Self::MAX_GAIN_DB),
        )
    }

    pub(super) fn track_peak_before_track_gain(&self) -> Option<f32> {
        let regions = self.region_edits();
        let mut cuts = vec![0.0, self.duration_sec];
        for region in &regions {
            let (effect_start, effect_end) = region.effect_bounds(self.duration_sec);
            cuts.extend([effect_start, region.start_sec, region.end_sec, effect_end]);
        }
        cuts.sort_by(f32::total_cmp);
        cuts.dedup_by(|a, b| (*a - *b).abs() < f32::EPSILON);
        let mut peak = 0.0_f32;
        for window in cuts.windows(2) {
            let (start, end) = (window[0], window[1]);
            let time = (start + end) * 0.5;
            let active = regions
                .iter()
                .find(|region| region.effect_mix(time, self.duration_sec) > 0.0);
            let Some(raw_peak) = self.peak_amplitude_in_range((start, end)) else {
                continue;
            };
            if let Some(region) = active {
                let wet_peak = apply_audio_gain(raw_peak, region.gain_db, region.soft_clip);
                for at in [start, time, end] {
                    let mix = region.effect_mix(at, self.duration_sec);
                    peak = peak.max((raw_peak + (wet_peak - raw_peak) * mix).abs());
                }
            } else {
                peak = peak.max(raw_peak);
            }
        }
        (peak.is_finite() && peak > f32::EPSILON).then_some(peak)
    }

    pub(super) fn normalize_track(&mut self, now: f64) {
        let Some(peak) = self.track_peak_before_track_gain() else {
            let message = if self.analysis.is_some() {
                "Track is silent; normalization made no change"
            } else {
                "Audio analysis is still loading"
            };
            self.status_toast = Some((message.into(), now));
            return;
        };
        let Some(gain_db) = Self::normalization_gain_db(peak, 0.0) else {
            self.status_toast = Some(("Could not measure the track peak".into(), now));
            return;
        };
        let before = self.edit_snapshot();
        self.track_gain_db = gain_db;
        self.record_edit(before);
        self.status_toast = Some((
            format!(
                "Normalized track to {:.1} dBFS (gain {gain_db:+.1} dB)",
                Self::NORMALIZE_TARGET_DBFS
            )
            .into(),
            now,
        ));
    }

    pub(super) fn normalize_active_region(&mut self, now: f64) {
        let Some(range) = self.active_selection_range() else {
            self.normalize_track(now);
            return;
        };
        let Some(peak) = self.peak_amplitude_in_range(range) else {
            let message = if self.analysis.is_some() {
                "Selected region is silent; normalization made no change"
            } else {
                "Audio analysis is still loading"
            };
            self.status_toast = Some((message.into(), now));
            return;
        };
        let Some(gain_db) = Self::normalization_gain_db(peak, self.track_gain_db) else {
            self.status_toast = Some(("Could not measure the selected peak".into(), now));
            return;
        };
        let before = self.edit_snapshot();
        let label = match self.selection_mode {
            SelectionMode::Single => {
                self.single_region_gain_db = gain_db;
                "selection".to_string()
            }
            SelectionMode::Multi => {
                let Some(index) = self.active_multi_index else {
                    self.normalize_track(now);
                    return;
                };
                self.region_gain_db.resize(self.multi_selections.len(), 0.0);
                self.region_gain_db[index] = gain_db;
                format!("region #{}", index + 1)
            }
        };
        self.record_edit(before);
        self.status_toast = Some((
            format!(
                "Normalized {label} to {:.1} dBFS (region gain {gain_db:+.1} dB)",
                Self::NORMALIZE_TARGET_DBFS
            )
            .into(),
            now,
        ));
    }

    pub(super) fn normalize_at_time(&mut self, hovered_time: Option<f32>, now: f64) {
        if hovered_time.is_some_and(|time| self.focus_region_at_time(time)) {
            self.normalize_active_region(now);
        } else {
            self.normalize_track(now);
        }
    }

    pub(super) fn crossfade_limit(range: (f32, f32), duration_sec: f32) -> f32 {
        let (start, end) = AudioSelectionModel::normalize(range);
        let left = start.clamp(0.0, duration_sec);
        let right = (duration_sec - end).clamp(0.0, duration_sec);
        match (left > 0.0, right > 0.0) {
            (true, true) => left.min(right),
            (true, false) => left,
            (false, true) => right,
            (false, false) => 0.0,
        }
    }

    pub(super) fn adjust_active_region_crossfade(&mut self, wheel_steps: f32, now: Instant) {
        let before = self.edit_snapshot();
        match self.selection_mode {
            SelectionMode::Single => {
                if let Some(range) = self.selection {
                    let limit = Self::crossfade_limit(range, self.duration_sec);
                    self.single_crossfade_sec =
                        (self.single_crossfade_sec + wheel_steps * 0.01).clamp(0.0, limit);
                }
            }
            SelectionMode::Multi => {
                if let Some(index) = self.active_multi_index
                    && let Some(&range) = self.multi_selections.get(index)
                {
                    let limit = Self::crossfade_limit(range, self.duration_sec);
                    self.region_crossfade_sec
                        .resize(self.multi_selections.len(), 0.0);
                    if let Some(crossfade) = self.region_crossfade_sec.get_mut(index) {
                        *crossfade = (*crossfade + wheel_steps * 0.01).clamp(0.0, limit);
                    }
                }
            }
        }
        self.record_scroll_edit(before, ScrollEditKind::Crossfade, now);
    }

    pub(super) fn active_region_crossfade(&self) -> f32 {
        let (range, crossfade) = match self.selection_mode {
            SelectionMode::Single => (self.selection, self.single_crossfade_sec),
            SelectionMode::Multi => {
                let range = self
                    .active_multi_index
                    .and_then(|index| self.multi_selections.get(index).copied());
                let crossfade = self
                    .active_multi_index
                    .and_then(|index| self.region_crossfade_sec.get(index).copied())
                    .unwrap_or(0.0);
                (range, crossfade)
            }
        };
        range
            .map(|range| crossfade.min(Self::crossfade_limit(range, self.duration_sec)))
            .unwrap_or(0.0)
    }

    pub(super) fn reset_active_region_crossfade(&mut self) -> bool {
        let before = self.edit_snapshot();
        let changed = match self.selection_mode {
            SelectionMode::Single if self.selection.is_some() => {
                let changed = self.single_crossfade_sec != 0.0;
                self.single_crossfade_sec = 0.0;
                changed
            }
            SelectionMode::Multi => self
                .active_multi_index
                .and_then(|index| self.region_crossfade_sec.get_mut(index))
                .map(|crossfade| {
                    let changed = *crossfade != 0.0;
                    *crossfade = 0.0;
                    changed
                })
                .unwrap_or(false),
            _ => false,
        };
        self.record_edit(before);
        changed
    }

    pub(super) fn filter_cutoff_range(&self) -> (f32, f32) {
        let sample_rate = self
            .analysis
            .as_ref()
            .map(|a| a.sample_rate)
            .or_else(|| self.metadata.as_ref().map(|m| m.sample_rate))
            .unwrap_or(44_100) as f32;
        let max_hz = if sample_rate > 196_000.0 {
            sample_rate * 0.5
        } else {
            (sample_rate * 0.45).min(20_000.0).max(20.0)
        };
        (20.0, max_hz.max(20.0))
    }

    pub(super) fn step_filter_cutoff(
        current: Option<f32>,
        wheel_steps: f32,
        kind: SpectrumFilterKind,
        range: (f32, f32),
        sample_rate: f32,
    ) -> Option<f32> {
        if wheel_steps == 0.0 {
            return current;
        }
        let start = match kind {
            SpectrumFilterKind::LowPass => {
                if sample_rate > 196_000.0 {
                    range.1
                } else {
                    range.1.min(12_000.0)
                }
            }
            SpectrumFilterKind::HighPass => 120.0_f32.clamp(range.0, range.1),
        };
        Some((current.unwrap_or(start) * 2.0_f32.powf(wheel_steps / 4.0)).clamp(range.0, range.1))
    }

    pub(super) fn adjust_active_region_filter(
        &mut self,
        kind: SpectrumFilterKind,
        wheel_steps: f32,
        now: Instant,
    ) {
        let before = self.edit_snapshot();
        let range = self.filter_cutoff_range();
        let sample_rate = self
            .analysis
            .as_ref()
            .map(|a| a.sample_rate)
            .or_else(|| self.metadata.as_ref().map(|m| m.sample_rate))
            .unwrap_or(44_100) as f32;
        match self.selection_mode {
            SelectionMode::Single if self.selection.is_some() => {
                let cutoff = match kind {
                    SpectrumFilterKind::LowPass => &mut self.single_low_pass_hz,
                    SpectrumFilterKind::HighPass => &mut self.single_high_pass_hz,
                };
                *cutoff = Self::step_filter_cutoff(*cutoff, wheel_steps, kind, range, sample_rate);
            }
            SelectionMode::Multi => {
                if let Some(index) = self.active_multi_index {
                    let cutoffs = match kind {
                        SpectrumFilterKind::LowPass => &mut self.region_low_pass_hz,
                        SpectrumFilterKind::HighPass => &mut self.region_high_pass_hz,
                    };
                    cutoffs.resize(self.multi_selections.len(), None);
                    if let Some(cutoff) = cutoffs.get_mut(index) {
                        *cutoff = Self::step_filter_cutoff(
                            *cutoff,
                            wheel_steps,
                            kind,
                            range,
                            sample_rate,
                        );
                    }
                }
            }
            _ => {}
        }
        self.record_scroll_edit(before, ScrollEditKind::Filter(kind), now);
    }

    pub(super) fn active_filter_cutoff(&self, kind: SpectrumFilterKind) -> Option<f32> {
        match (self.selection_mode, kind) {
            (SelectionMode::Single, SpectrumFilterKind::LowPass) => self.single_low_pass_hz,
            (SelectionMode::Single, SpectrumFilterKind::HighPass) => self.single_high_pass_hz,
            (SelectionMode::Multi, SpectrumFilterKind::LowPass) => self
                .active_multi_index
                .and_then(|i| self.region_low_pass_hz.get(i).copied().flatten()),
            (SelectionMode::Multi, SpectrumFilterKind::HighPass) => self
                .active_multi_index
                .and_then(|i| self.region_high_pass_hz.get(i).copied().flatten()),
        }
    }

    pub(super) fn reset_active_region_filter(&mut self, kind: SpectrumFilterKind) -> bool {
        let before = self.edit_snapshot();
        let cutoff = match (self.selection_mode, kind) {
            (SelectionMode::Single, SpectrumFilterKind::LowPass) if self.selection.is_some() => {
                Some(&mut self.single_low_pass_hz)
            }
            (SelectionMode::Single, SpectrumFilterKind::HighPass) if self.selection.is_some() => {
                Some(&mut self.single_high_pass_hz)
            }
            (SelectionMode::Multi, SpectrumFilterKind::LowPass) => self
                .active_multi_index
                .and_then(|i| self.region_low_pass_hz.get_mut(i)),
            (SelectionMode::Multi, SpectrumFilterKind::HighPass) => self
                .active_multi_index
                .and_then(|i| self.region_high_pass_hz.get_mut(i)),
            _ => None,
        };
        let changed = if let Some(cutoff) = cutoff {
            cutoff.take().is_some()
        } else {
            false
        };
        self.record_edit(before);
        changed
    }

    pub(super) fn clear_single_region_effects(&mut self) {
        self.single_region_gain_db = 0.0;
        self.single_crossfade_sec = 0.0;
        self.single_low_pass_hz = None;
        self.single_high_pass_hz = None;
    }

    pub(super) fn clear_multi_regions(&mut self) {
        self.multi_selections.clear();
        self.region_gain_db.clear();
        self.region_crossfade_sec.clear();
        self.region_low_pass_hz.clear();
        self.region_high_pass_hz.clear();
        self.active_multi_index = None;
    }

    pub(super) fn remove_multi_region(&mut self, index: usize) {
        if index >= self.multi_selections.len() {
            return;
        }
        self.multi_selections.remove(index);
        if index < self.region_gain_db.len() {
            self.region_gain_db.remove(index);
        }
        if index < self.region_crossfade_sec.len() {
            self.region_crossfade_sec.remove(index);
        }
        if index < self.region_low_pass_hz.len() {
            self.region_low_pass_hz.remove(index);
        }
        if index < self.region_high_pass_hz.len() {
            self.region_high_pass_hz.remove(index);
        }
        self.active_multi_index = match self.active_multi_index {
            Some(active) if active == index => None,
            Some(active) if active > index => Some(active - 1),
            active => active,
        };
    }

    pub(super) fn sort_multi_regions(&mut self) {
        let mut order: Vec<usize> = (0..self.multi_selections.len()).collect();
        order.sort_by(|&a, &b| {
            self.multi_selections[a]
                .0
                .total_cmp(&self.multi_selections[b].0)
        });
        let active = self.active_multi_index;
        let regions = self.multi_selections.clone();
        let gains = self.region_gain_db.clone();
        let crossfades = self.region_crossfade_sec.clone();
        let low = self.region_low_pass_hz.clone();
        let high = self.region_high_pass_hz.clone();
        self.multi_selections = order.iter().map(|&i| regions[i]).collect();
        self.region_gain_db = order
            .iter()
            .map(|&i| gains.get(i).copied().unwrap_or(0.0))
            .collect();
        self.region_crossfade_sec = order
            .iter()
            .map(|&i| crossfades.get(i).copied().unwrap_or(0.0))
            .collect();
        self.region_low_pass_hz = order
            .iter()
            .map(|&i| low.get(i).copied().flatten())
            .collect();
        self.region_high_pass_hz = order
            .iter()
            .map(|&i| high.get(i).copied().flatten())
            .collect();
        self.active_multi_index = active.and_then(|old| order.iter().position(|&i| i == old));
    }

    pub(super) fn focus_region_at_time(&mut self, time: f32) -> bool {
        match self.selection_mode {
            SelectionMode::Single => self
                .selection
                .is_some_and(|(start, end)| time >= start.min(end) && time <= start.max(end)),
            SelectionMode::Multi => {
                self.active_multi_index = self
                    .multi_selections
                    .iter()
                    .position(|&(start, end)| time >= start.min(end) && time <= start.max(end));
                self.active_multi_index.is_some()
            }
        }
    }

    pub(super) fn reset_active_region_gain(&mut self) -> bool {
        let before = self.edit_snapshot();
        let changed = match self.selection_mode {
            SelectionMode::Single if self.selection.is_some() => {
                let changed = self.single_region_gain_db != 0.0;
                self.single_region_gain_db = 0.0;
                changed
            }
            SelectionMode::Multi => self
                .active_multi_index
                .and_then(|index| self.region_gain_db.get_mut(index))
                .map(|gain| {
                    let changed = *gain != 0.0;
                    *gain = 0.0;
                    changed
                })
                .unwrap_or(false),
            _ => false,
        };
        self.record_edit(before);
        changed
    }

    pub(super) fn region_edit(
        &self,
        start: f32,
        end: f32,
        crossfade_sec: f32,
        gain_db: f32,
        low_pass_hz: Option<f32>,
        high_pass_hz: Option<f32>,
    ) -> AudioRegionEdit {
        let crossfade_sec = crossfade_sec
            .max(0.0)
            .min(Self::crossfade_limit((start, end), self.duration_sec));
        let effective_gain_db = Self::combined_gain_db(self.track_gain_db, gain_db);
        let soft_clip = if effective_gain_db <= 0.0 {
            false
        } else if let Some(analysis) = &self.analysis {
            let sample_rate = analysis.sample_rate as f32;
            let first =
                ((start.min(end).max(0.0) * sample_rate) as usize).min(analysis.total_frames);
            let last =
                ((start.max(end).max(0.0) * sample_rate) as usize).min(analysis.total_frames);
            if first >= last || analysis.channels > 2 {
                true
            } else {
                let mut peak = 0.0_f32;
                for ch in 0..if analysis.ch2_samples.is_some() { 2 } else { 1 } {
                    let (min, max, _) = analysis.query_peaks(ch, first, last);
                    peak = peak.max(min.abs()).max(max.abs());
                }
                peak * 10.0_f32.powf(effective_gain_db / 20.0) > 1.0
            }
        } else {
            true
        };
        AudioRegionEdit {
            start_sec: start.min(end),
            end_sec: start.max(end),
            crossfade_sec,
            gain_db,
            soft_clip,
            low_pass_hz,
            high_pass_hz,
        }
    }

    pub(super) fn active_region_edit(&self) -> Option<AudioRegionEdit> {
        match self.selection_mode {
            SelectionMode::Single => self.selection.map(|(start, end)| {
                self.region_edit(
                    start,
                    end,
                    self.single_crossfade_sec,
                    self.single_region_gain_db,
                    self.single_low_pass_hz,
                    self.single_high_pass_hz,
                )
            }),
            SelectionMode::Multi => self
                .active_multi_index
                .or_else(|| (!self.multi_selections.is_empty()).then_some(0))
                .and_then(|index| {
                    self.multi_selections.get(index).map(|&(start, end)| {
                        self.region_edit(
                            start,
                            end,
                            self.region_crossfade_sec.get(index).copied().unwrap_or(0.0),
                            self.region_gain_db.get(index).copied().unwrap_or(0.0),
                            self.region_low_pass_hz.get(index).copied().flatten(),
                            self.region_high_pass_hz.get(index).copied().flatten(),
                        )
                    })
                }),
        }
    }

    pub(super) fn region_edits(&self) -> Vec<AudioRegionEdit> {
        let regions: Vec<AudioRegionEdit> = match self.selection_mode {
            SelectionMode::Single => self
                .selection
                .into_iter()
                .map(|(start, end)| {
                    self.region_edit(
                        start,
                        end,
                        self.single_crossfade_sec,
                        self.single_region_gain_db,
                        self.single_low_pass_hz,
                        self.single_high_pass_hz,
                    )
                })
                .collect(),
            SelectionMode::Multi => self
                .multi_selections
                .iter()
                .enumerate()
                .map(|(index, &(start, end))| {
                    let gain_db = self.region_gain_db.get(index).copied().unwrap_or(0.0);
                    self.region_edit(
                        start,
                        end,
                        self.region_crossfade_sec.get(index).copied().unwrap_or(0.0),
                        gain_db,
                        self.region_low_pass_hz.get(index).copied().flatten(),
                        self.region_high_pass_hz.get(index).copied().flatten(),
                    )
                })
                .collect(),
        };
        if regions.iter().any(|region: &AudioRegionEdit| {
            region.gain_db != 0.0 || region.low_pass_hz.is_some() || region.high_pass_hz.is_some()
        }) {
            regions
        } else {
            Vec::new()
        }
    }

    pub(super) fn session_audio_edits(&self) -> SessionAudioEdits {
        SessionAudioEdits {
            track_gain_db: self.track_gain_db,
            regions: self.region_edits(),
        }
    }

    pub(super) fn combined_gain_db(track_gain_db: f32, region_gain_db: f32) -> f32 {
        if track_gain_db == f32::NEG_INFINITY || region_gain_db == f32::NEG_INFINITY {
            f32::NEG_INFINITY
        } else {
            track_gain_db + region_gain_db
        }
    }

    pub(super) fn export_region_edit(
        &self,
        start: f32,
        end: f32,
        crossfade_sec: f32,
        region_gain_db: f32,
        low_pass_hz: Option<f32>,
        high_pass_hz: Option<f32>,
    ) -> AudioRegionEdit {
        let mut region = self.region_edit(
            start,
            end,
            crossfade_sec,
            region_gain_db,
            low_pass_hz,
            high_pass_hz,
        );
        region.gain_db = Self::combined_gain_db(self.track_gain_db, region_gain_db);
        region
    }

    pub(super) fn gain_filter(gain_db: f32, soft_clip: bool) -> Option<String> {
        if gain_db == 0.0 {
            None
        } else if gain_db == f32::NEG_INFINITY {
            Some("volume=0".to_string())
        } else if soft_clip {
            Some(format!("volume={gain_db}dB,asoftclip=type=tanh"))
        } else {
            Some(format!("volume={gain_db}dB"))
        }
    }

    pub(super) fn region_filter(region: AudioRegionEdit) -> Option<String> {
        let mut filters = Vec::new();
        if let Some(gain) = Self::gain_filter(region.gain_db, region.soft_clip) {
            filters.push(gain);
        }
        if let Some(cutoff) = region.low_pass_hz {
            filters.push(format!("lowpass=f={cutoff:.3}:p=2"));
        }
        if let Some(cutoff) = region.high_pass_hz {
            filters.push(format!("highpass=f={cutoff:.3}:p=2"));
        }
        (!filters.is_empty()).then(|| filters.join(","))
    }

    pub(super) fn channel_edit_filter(
        mode: ChannelCopyMode,
        region: AudioRegionEdit,
    ) -> Option<String> {
        let pan = match mode {
            ChannelCopyMode::Native => None,
            ChannelCopyMode::LeftOnly => Some("pan=mono|c0=c0"),
            ChannelCopyMode::RightOnly => Some("pan=mono|c0=c1"),
        };
        match (pan, Self::region_filter(region)) {
            (Some(pan), Some(gain)) => Some(format!("{pan},{gain}")),
            (Some(pan), None) => Some(pan.to_string()),
            (None, gain) => gain,
        }
    }

    /// Trims leading and trailing silence on both sides by detecting signal boundaries and setting the active selection.
    pub fn trim_silence(&mut self, threshold_db: f32, now: f64) {
        let Some(ref analysis) = self.analysis else {
            self.status_toast = Some((
                crate::ui_ext::StatusMessage::new(
                    crate::icons::Icon::Alert,
                    "Audio analysis still loading...".to_string(),
                ),
                now,
            ));
            return;
        };
        if let Some((start_sec, end_sec)) = analysis.detect_silence_bounds(threshold_db) {
            let dur = end_sec - start_sec;
            self.selection_mode = SelectionMode::Single;
            self.selection = Some((start_sec, end_sec));
            self.clear_single_region_effects();
            self.current_time_sec = start_sec;
            self.status_toast = Some((
                crate::ui_ext::StatusMessage::new(
                    crate::icons::Icon::Scissors,
                    format!(
                        "Trimmed silence: {:.2}s - {:.2}s ({:.2}s duration)",
                        start_sec, end_sec, dur
                    ),
                ),
                now,
            ));
        } else {
            self.status_toast = Some((
                crate::ui_ext::StatusMessage::new(
                    crate::icons::Icon::Alert,
                    format!(
                        "No audio signal detected above silence threshold ({:.0}dB)",
                        threshold_db
                    ),
                ),
                now,
            ));
        }
    }

    /// Removes the active slice into a private session WAV. The original file is
    /// never changed; subsequent playback, waveform analysis, export and the
    /// Audacity escape hatch all operate on the squished session timeline.
    pub(super) fn delete_active_slice_from_session(&mut self, now: f64) {
        let Some((start, end)) = self.active_selection_range() else {
            self.status_toast = Some(("Hover/select a slice before deleting it".into(), now));
            return;
        };
        let start = start.min(end).max(0.0);
        let end = end.max(start).min(self.duration_sec);
        if end - start < 0.001 || end >= self.duration_sec && start <= 0.0 {
            self.status_toast = Some(("Cannot delete the entire session".into(), now));
            return;
        }
        let out = std::env::temp_dir()
            .join("little_clip_audio_sessions")
            .join(format!("edited-{}.wav", uuid::Uuid::new_v4()));
        if let Some(parent) = out.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                tracing::warn!(
                    "Failed to create audio session directory {}: {err}",
                    parent.display()
                );
                self.status_toast = Some(("Failed to create session directory".into(), now));
                return;
            }
        }
        let mut filters = Vec::new();
        let mut labels = Vec::new();
        if start > 0.001 {
            filters.push(format!("[0:a]atrim=0:{start:.6},asetpts=PTS-STARTPTS[a0]"));
            labels.push("[a0]");
        }
        if end < self.duration_sec - 0.001 {
            filters.push(format!("[0:a]atrim={end:.6},asetpts=PTS-STARTPTS[a1]"));
            labels.push("[a1]");
        }
        let filter = if labels.len() == 2 {
            format!(
                "{};{}concat=n=2:v=0:a=1[out]",
                filters.join(";"),
                labels.join("")
            )
        } else {
            format!("{};{}anull[out]", filters.join(";"), labels[0])
        };
        let ok = std::process::Command::new("ffmpeg")
            .args(["-y", "-i"])
            .arg(&self.path)
            .args([
                "-filter_complex",
                &filter,
                "-map",
                "[out]",
                "-c:a",
                "pcm_s16le",
            ])
            .arg(&out)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok || !out.is_file() {
            self.status_toast = Some(("Failed to create edited audio session".into(), now));
            return;
        }
        let before = self.edit_snapshot();
        self.stop();
        self.path = out;
        self.duration_sec = (self.duration_sec - (end - start)).max(0.0);
        self.view_start_sec = 0.0;
        self.view_duration_sec = self.duration_sec;
        self.current_time_sec = start.min(self.duration_sec);
        self.selection = None;
        self.track_gain_db = 0.0;
        self.clear_multi_regions();
        self.clear_single_region_effects();
        self.restart_session_analysis();
        self.record_edit(before);
        self.status_toast = Some((
            format!("Deleted {:.2}s; waveform recomputing", end - start).into(),
            now,
        ));
    }
}
