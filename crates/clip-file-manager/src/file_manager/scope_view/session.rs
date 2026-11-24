use super::{
    MarkedRange, Scope, ScopeTrace, Viewer, filters::SavedFilter, spectrum_input::DisplaySettings,
};
use little_clip_core::engineering::analysis::SpectrumWindow;
use serde::{Deserialize, Serialize};
use std::ops::Range;

#[derive(PartialEq)]
pub(in crate::file_manager) struct AnalysisEdits {
    math: String,
    digital: super::digital::Settings,
    filters: Vec<SavedFilter>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(in crate::file_manager) struct Settings {
    active: usize,
    cursors: [Option<f64>; 2],
    #[serde(default, skip_serializing_if = "Scope::legacy")]
    cursor_scope: Scope<ScopeTrace>,
    marks: Vec<MarkedRange>,
    stacked: bool,
    visible: Vec<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    channel_order: Vec<usize>,
    display_offsets: Vec<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    fft_traces: Vec<ScopeTrace>,
    math: String,
    #[serde(default)]
    digital: Option<super::digital::Settings>,
    filters: Vec<SavedFilter>,
    spectrum: bool,
    spectra: DisplaySettings,
    spectrum_window: SpectrumWindow,
    spectrum_db: bool,
    spectrum_marker: Option<f64>,
    spectrum_span: Option<Range<f64>>,
}

impl Settings {
    pub(in crate::file_manager) fn invalidate_archived_peaks(&mut self) {
        for mark in &mut self.marks {
            mark.peaks = Default::default();
            mark.legacy_channel_peaks.clear();
            mark.legacy_peak = None;
        }
    }
}

impl Viewer {
    pub(in crate::file_manager) fn analysis_edits(&self) -> AnalysisEdits {
        AnalysisEdits {
            math: self.math.clone(),
            digital: self.digital.analysis_edits(),
            filters: self.filters.analysis_edits(),
        }
    }

    pub(in crate::file_manager) fn session_settings(&self) -> Settings {
        Settings {
            active: self.active,
            cursors: self.cursors,
            cursor_scope: self.cursor_scope.clone(),
            marks: self.marks.clone(),
            stacked: self.stacked,
            visible: self.visible.clone(),
            channel_order: self.channel_order.clone(),
            display_offsets: self.display_offsets.clone(),
            fft_traces: self.trace_fft.keys().copied().collect(),
            math: self.math.clone(),
            digital: Some(self.digital.save()),
            filters: self.filters.save(),
            spectrum: self.spectrum,
            spectra: self.spectra.settings(),
            spectrum_window: self.spectrum_window,
            spectrum_db: self.spectrum_db,
            spectrum_marker: self.spectrum_marker,
            spectrum_span: self.spectrum_span.clone(),
        }
    }

    pub(in crate::file_manager) fn restore_session(
        &mut self,
        state: Settings,
    ) -> Result<(), String> {
        let count = self.overview.channels.len();
        if state.active >= count
            || state.visible.len() != count
            || state.display_offsets.len() != count
            || state.fft_traces.iter().any(|trace| !trace.valid(count))
        {
            return Err("The saved scope channels do not match this recording".into());
        }
        if state.marks.iter().any(|mark| mark.span.is_empty()) {
            return Err("Invalid scope markers".into());
        }
        self.filters
            .restore(state.filters, &self.overview.channels)?;
        self.active = state.active;
        self.cursors = state.cursors;
        self.cursor_scope = state.cursor_scope;
        self.marks = state.marks;
        for mark in &mut self.marks {
            let legacy = std::mem::take(&mut mark.legacy_channel_peaks);
            if mark.peaks.channel_peaks.is_empty() {
                mark.peaks.channel_peaks = legacy;
            }
        }
        self.stacked = state.stacked
            || self
                .overview
                .channels
                .windows(2)
                .any(|pair| pair[0].unit != pair[1].unit);
        self.visible = state.visible;
        let mut ordered = state.channel_order.clone();
        ordered.sort_unstable();
        if ordered.into_iter().eq(0..count) {
            self.channel_order = state.channel_order;
        }
        self.display_offsets = state.display_offsets;
        self.trace_fft = state
            .fft_traces
            .into_iter()
            .map(|index| (index, Default::default()))
            .collect();
        self.math = state.math;
        if let Some(digital) = state.digital {
            self.digital.restore(digital)?;
        }
        self.spectrum = state.spectrum;
        self.spectra.restore_settings(state.spectra);
        self.spectrum_window = state.spectrum_window;
        self.spectrum_db = state.spectrum_db;
        self.spectrum_marker = state.spectrum_marker;
        self.spectrum_span = state.spectrum_span.filter(|span| !span.is_empty());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use little_clip_core::engineering::analysis::FilterKind;
    use std::{
        fs,
        sync::atomic::AtomicBool,
        thread,
        time::{Duration, Instant},
    };

    const FIXTURE_LANES: [ScopeTrace; 3] = [
        ScopeTrace::Recorded(0),
        ScopeTrace::Math,
        ScopeTrace::Filtered(0),
    ];

    fn recording_fixture(path: &std::path::Path) {
        use std::fmt::Write;
        let mut data = String::from("X,CH2,CH4,Start,Increment,\nSequence,Volt,Volt,0,1e-3,\n");
        for row in 0..128 {
            let value = (row as f32 * 0.1).sin();
            writeln!(&mut data, "{row},{value},{},,,", 2.0 - value).unwrap();
        }
        fs::write(path, data).unwrap();
    }

    fn settle_filters(view: &mut Viewer, ctx: &egui::Context, expected: &[usize]) {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            let _ = clip_ui::test_support::run_frame(
                ctx,
                egui::RawInput {
                    time: Some(start.elapsed().as_secs_f64()),
                    ..Default::default()
                },
                |ctx| {
                    ctx.root_ui(|ui| {
                        egui::CentralPanel::default().show(ui, |ui| {
                            view.filters.update(ui, &view.recording, view.active);
                        });
                    });
                },
            );
            let ready: Vec<_> = view
                .filtered_channels()
                .filter(|lane| lane.samples.is_some())
                .map(|lane| lane.channel)
                .collect();
            if ready == expected {
                return;
            }
            thread::sleep(Duration::from_millis(15));
        }
        panic!("scope filters did not settle");
    }

    impl Viewer {
        pub(in crate::file_manager) fn archive_fixture_range(&mut self) {
            let ctx = egui::Context::default();
            self.math = "CH2 - CH4".into();
            self.math_state
                .compile(&self.math, self.active, &self.overview.channels);
            self.add_filter(FilterKind::LowPass);
            settle_filters(self, &ctx, &[0]);

            self.cursors = [Some(0.02), Some(0.10)];
            self.cursor_scope = Scope::new(FIXTURE_LANES.to_vec(), Some(2));
            self.request_analysis(&ctx);
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.analysis_job.is_some() {
                self.poll(&ctx);
                assert!(
                    Instant::now() < deadline,
                    "scope range measurement did not settle"
                );
                thread::sleep(Duration::from_millis(5));
            }
            self.archive_mark();
        }
    }

    #[test]
    fn archived_derived_peaks_follow_their_inputs_after_edit_and_recipe_reopen() {
        use crate::file_manager::signal_view::session::save_and_reopen_scope_fixture;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rigol.csv");
        recording_fixture(&path);
        let cancel = AtomicBool::new(false);
        let mut view = Viewer::load(&path, &cancel).unwrap();
        view.archive_fixture_range();
        let mut view = save_and_reopen_scope_fixture(
            view,
            &directory.path().join("original.clip-analysis"),
            &cancel,
        );
        view.math_state
            .compile(&view.math, view.active, &view.overview.channels);
        let value = |view: &Viewer, trace| {
            view.range_peak(&view.marks[0].peaks, trace)
                .map(|peak| (peak.amplitude, peak.observed))
        };
        let raw = value(&view, ScopeTrace::Recorded(0)).unwrap();
        assert!(value(&view, ScopeTrace::Math).is_some());
        let filtered = value(&view, ScopeTrace::Filtered(0)).unwrap();
        let original_unit = view.math_unit();

        view.math = "CH2 / CH4".into();
        // The old compiled expression can still be present during an input edit.
        assert_eq!(value(&view, ScopeTrace::Math), None);
        view.math_state
            .compile(&view.math, view.active, &view.overview.channels);
        assert!(view.math_state.numeric().is_some());
        assert_ne!(view.math_unit(), original_unit);
        assert_eq!(value(&view, ScopeTrace::Math), None);
        assert_eq!(value(&view, ScopeTrace::Filtered(0)), Some(filtered));

        view.add_filter(FilterKind::HighPass);
        assert_eq!(value(&view, ScopeTrace::Filtered(0)), None);
        let mut reopened = save_and_reopen_scope_fixture(
            view,
            &directory.path().join("edited.clip-analysis"),
            &cancel,
        );
        reopened
            .math_state
            .compile(&reopened.math, reopened.active, &reopened.overview.channels);
        let mark = &reopened.marks[0];
        assert_eq!(mark.span, 0.02..0.10);
        assert_eq!(mark.scope.lanes.as_deref(), Some(FIXTURE_LANES.as_slice()));
        assert_eq!(mark.scope.color, Some(2));
        assert_eq!(value(&reopened, ScopeTrace::Recorded(0)), Some(raw));
        assert_eq!(value(&reopened, ScopeTrace::Filtered(0)), None);
        assert_eq!(value(&reopened, ScopeTrace::Math), None);
    }

    #[test]
    fn old_recipe_ranges_keep_raw_peaks_without_unattributed_derived_values() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rigol.csv");
        recording_fixture(&path);
        let cancel = AtomicBool::new(false);
        let mut view = Viewer::load(&path, &cancel).unwrap();
        let mut saved = serde_json::to_value(view.session_settings()).unwrap();
        saved["marks"] = serde_json::json!([{
            "span": { "start": 0.02, "end": 0.10 },
            "channel_peaks": [{ "amplitude": 1.5, "observed": false }, null],
            "math_peak": { "amplitude": 2.5, "observed": false },
            "filter_peaks": [[0, { "amplitude": 0.75, "observed": true }]]
        }]);
        view.restore_session(serde_json::from_value(saved).unwrap())
            .unwrap();
        let peaks = &view.marks[0].peaks;
        assert_eq!(
            view.range_peak(peaks, ScopeTrace::Recorded(0))
                .unwrap()
                .amplitude,
            1.5
        );
        assert!(view.range_peak(peaks, ScopeTrace::Filtered(0)).is_none());
        assert!(view.range_peak(peaks, ScopeTrace::Math).is_none());
        assert_eq!(view.marks[0].span, 0.02..0.10);
    }

    #[test]
    fn restores_scope_display_and_recomputes_all_saved_filters() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rigol.csv");
        recording_fixture(&path);
        let cancel = AtomicBool::new(false);
        let mut view = Viewer::load(&path, &cancel).unwrap();
        view.stacked = true;
        view.visible[1] = false;
        view.display_offsets[0] = 0.25;
        view.cursors = [Some(0.0625), Some(0.125)];
        view.cursor_scope = Scope::new(vec![ScopeTrace::Recorded(0)], Some(2));
        view.marks.push(MarkedRange {
            span: 0.0625..0.125,
            scope: Default::default(),
            peaks: Default::default(),
            legacy_channel_peaks: Vec::new(),
            legacy_peak: Some((0.5, "V".into())),
        });
        view.add_filter(FilterKind::LowPass);
        view.active = 1;
        view.add_filter(FilterKind::HighPass);
        view.active = 0;

        let old_settings = serde_json::to_value(view.session_settings()).unwrap();
        assert!(
            old_settings["filters"][0]["settings"]
                .get("offset")
                .is_none()
        );
        let mut updated_settings = old_settings;
        updated_settings["filters"][0]["settings"]["offset"] = serde_json::json!(0.5);
        updated_settings["filters"][0]["settings"]["visible"] = serde_json::json!(false);
        let original = serde_json::to_string(&updated_settings).unwrap();
        let mut reopened = Viewer::load(&path, &cancel).unwrap();
        reopened
            .restore_session(serde_json::from_str(&original).unwrap())
            .unwrap();
        assert_eq!(
            serde_json::to_value(reopened.session_settings()).unwrap(),
            updated_settings
        );
        assert!(reopened.analysis_edits() == view.analysis_edits());

        settle_filters(&mut reopened, &egui::Context::default(), &[0, 1]);
    }
}
