use super::*;

impl AudioPreviewState {
    #[inline]
    pub fn is_time_selected(&self, t: f32) -> bool {
        match self.selection_mode {
            SelectionMode::Single => self
                .selection
                .map(|r| AudioSelectionModel::contains(r, t))
                .unwrap_or(false),
            SelectionMode::Multi => self
                .multi_selections
                .iter()
                .any(|&r| AudioSelectionModel::contains(r, t)),
        }
    }

    #[inline]
    pub fn active_selection_range(&self) -> Option<(f32, f32)> {
        let raw = match self.selection_mode {
            SelectionMode::Single => self.selection,
            SelectionMode::Multi => self
                .active_multi_index
                .and_then(|idx| self.multi_selections.get(idx).copied())
                .or_else(|| self.multi_selections.first().copied()),
        }?;
        Some(AudioSelectionModel::clamp_to_duration(
            AudioSelectionModel::normalize(raw),
            self.duration_sec,
        ))
    }

    pub fn snap_time(&self, time_sec: f32, bypass: bool) -> f32 {
        if bypass {
            return time_sec;
        }
        if let Some(ref analysis) = self.analysis {
            if self.show_transients {
                // Magnetic snapping radius: ~12px in screen space converted to seconds
                let snap_radius = (self.view_duration_sec / 800.0) * 12.0;
                let mut nearest_transient = None;
                let mut nearest_distance = snap_radius;
                for &transient in self.current_transient_markers() {
                    let distance = (transient - time_sec).abs();
                    if distance <= nearest_distance {
                        nearest_distance = distance;
                        nearest_transient = Some(transient);
                    }
                }
                if let Some(t_transient) = nearest_transient {
                    return if self.snap_zero_crossings {
                        analysis.snap_to_zero_crossing(t_transient, 500)
                    } else {
                        t_transient
                    };
                }
            }
            if self.snap_zero_crossings {
                return analysis.snap_to_zero_crossing(time_sec, 500);
            }
        }
        time_sec
    }

    pub fn snap_active_selection_to_zero_crossings(&mut self, now: f64) {
        let Some(ref analysis) = self.analysis else {
            return;
        };
        match self.selection_mode {
            SelectionMode::Single => {
                if let Some((s, e)) = self.selection {
                    let s_snapped = analysis.snap_to_zero_crossing(s, 500);
                    let e_snapped = analysis.snap_to_zero_crossing(e, 500);
                    let (s_new, e_new) = (s_snapped.min(e_snapped), s_snapped.max(e_snapped));
                    self.selection = Some((s_new, e_new));
                    self.status_toast = Some((
                        crate::ui_ext::StatusMessage::new(
                            crate::icons::Icon::Waves,
                            format!(
                                "Snapped selection to 0-crossings: {:.4}s - {:.4}s",
                                s_new, e_new
                            ),
                        ),
                        now,
                    ));
                }
            }
            SelectionMode::Multi => {
                if let Some(idx) = self.active_multi_index {
                    if idx < self.multi_selections.len() {
                        let (s, e) = self.multi_selections[idx];
                        let s_snapped = analysis.snap_to_zero_crossing(s, 500);
                        let e_snapped = analysis.snap_to_zero_crossing(e, 500);
                        let (s_new, e_new) = (s_snapped.min(e_snapped), s_snapped.max(e_snapped));
                        self.multi_selections[idx] = (s_new, e_new);
                        self.status_toast = Some((
                            crate::ui_ext::StatusMessage::new(
                                crate::icons::Icon::Waves,
                                format!(
                                    "Snapped region #{} to 0-crossings: {:.4}s - {:.4}s",
                                    idx + 1,
                                    s_new,
                                    e_new
                                ),
                            ),
                            now,
                        ));
                    }
                } else if !self.multi_selections.is_empty() {
                    for region in &mut self.multi_selections {
                        let s_snapped = analysis.snap_to_zero_crossing(region.0, 500);
                        let e_snapped = analysis.snap_to_zero_crossing(region.1, 500);
                        *region = (s_snapped.min(e_snapped), s_snapped.max(e_snapped));
                    }
                    self.status_toast = Some((
                        crate::ui_ext::StatusMessage::new(
                            crate::icons::Icon::Waves,
                            format!(
                                "Snapped all {} regions to 0-crossings",
                                self.multi_selections.len()
                            ),
                        ),
                        now,
                    ));
                }
            }
        }
    }

    pub fn set_selection_mode(&mut self, mode: SelectionMode) {
        if self.selection_mode == mode {
            return;
        }
        match mode {
            SelectionMode::Multi => {
                if let Some(sel) = self.selection {
                    let index = if let Some(index) =
                        self.multi_selections.iter().position(|range| *range == sel)
                    {
                        index
                    } else {
                        self.multi_selections.push(sel);
                        self.multi_selections.len() - 1
                    };
                    self.region_gain_db.resize(self.multi_selections.len(), 0.0);
                    self.region_gain_db[index] = self.single_region_gain_db;
                    self.region_crossfade_sec
                        .resize(self.multi_selections.len(), 0.0);
                    self.region_crossfade_sec[index] = self.single_crossfade_sec;
                    self.region_low_pass_hz
                        .resize(self.multi_selections.len(), None);
                    self.region_high_pass_hz
                        .resize(self.multi_selections.len(), None);
                    self.region_low_pass_hz[index] = self.single_low_pass_hz;
                    self.region_high_pass_hz[index] = self.single_high_pass_hz;
                    self.active_multi_index = Some(index);
                }
            }
            SelectionMode::Single => {
                if let Some(idx) = self.active_multi_index {
                    if idx < self.multi_selections.len() {
                        self.selection = Some(self.multi_selections[idx]);
                        self.single_region_gain_db =
                            self.region_gain_db.get(idx).copied().unwrap_or(0.0);
                        self.single_crossfade_sec =
                            self.region_crossfade_sec.get(idx).copied().unwrap_or(0.0);
                        self.single_low_pass_hz =
                            self.region_low_pass_hz.get(idx).copied().flatten();
                        self.single_high_pass_hz =
                            self.region_high_pass_hz.get(idx).copied().flatten();
                    }
                } else if let Some(first) = self.multi_selections.first() {
                    self.selection = Some(*first);
                    self.single_region_gain_db =
                        self.region_gain_db.first().copied().unwrap_or(0.0);
                    self.single_crossfade_sec =
                        self.region_crossfade_sec.first().copied().unwrap_or(0.0);
                    self.single_low_pass_hz = self.region_low_pass_hz.first().copied().flatten();
                    self.single_high_pass_hz = self.region_high_pass_hz.first().copied().flatten();
                }
            }
        }
        self.selection_mode = mode;
    }

    pub fn zoom(&mut self, factor: f32, anchor_time: f32) {
        let sample_rate = self
            .analysis
            .as_ref()
            .map(|a| a.sample_rate as f32)
            .unwrap_or(44100.0);
        let mut vp = AudioViewport {
            start_sec: self.view_start_sec,
            duration_sec: self.view_duration_sec,
        };
        vp.zoom(factor, anchor_time, self.duration_sec, sample_rate);
        self.view_start_sec = vp.start_sec;
        self.view_duration_sec = vp.duration_sec;
    }

    pub fn pan(&mut self, delta_sec: f32) {
        let mut vp = AudioViewport {
            start_sec: self.view_start_sec,
            duration_sec: self.view_duration_sec,
        };
        vp.pan(delta_sec, self.duration_sec);
        self.view_start_sec = vp.start_sec;
    }

    pub fn fit_to_window(&mut self) {
        let mut vp = AudioViewport {
            start_sec: self.view_start_sec,
            duration_sec: self.view_duration_sec,
        };
        vp.fit_to_window(self.duration_sec);
        self.view_start_sec = vp.start_sec;
        self.view_duration_sec = vp.duration_sec;
    }
}
