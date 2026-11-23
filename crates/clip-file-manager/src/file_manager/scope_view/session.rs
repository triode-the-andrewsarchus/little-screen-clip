use super::{MarkedRange, Viewer, filters::SavedFilter, spectrum_input::DisplaySettings};
use little_clip_core::engineering::analysis::SpectrumWindow;
use serde::{Deserialize, Serialize};
use std::ops::Range;

#[derive(Clone, Serialize, Deserialize)]
pub(in crate::file_manager) struct Settings {
    active: usize,
    cursors: [Option<f64>; 2],
    marks: Vec<MarkedRange>,
    stacked: bool,
    visible: Vec<bool>,
    display_offsets: Vec<f32>,
    math: String,
    filters: Vec<SavedFilter>,
    spectrum: bool,
    spectra: DisplaySettings,
    spectrum_window: SpectrumWindow,
    spectrum_db: bool,
    spectrum_marker: Option<f64>,
    spectrum_span: Option<Range<f64>>,
}

impl Viewer {
    pub(in crate::file_manager) fn session_settings(&self) -> Settings {
        Settings {
            active: self.active,
            cursors: self.cursors,
            marks: self.marks.clone(),
            stacked: self.stacked,
            visible: self.visible.clone(),
            display_offsets: self.display_offsets.clone(),
            math: self.math.clone(),
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
        self.marks = state.marks;
        self.stacked = state.stacked
            || self
                .overview
                .channels
                .windows(2)
                .any(|pair| pair[0].unit != pair[1].unit);
        self.visible = state.visible;
        self.display_offsets = state.display_offsets;
        self.math = state.math;
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

    #[test]
    fn restores_scope_display_and_recomputes_all_saved_filters() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("rigol.csv");
        let mut data = String::from("X,CH2,CH4,Start,Increment,\nSequence,Volt,Volt,0,1e-3,\n");
        for row in 0..128 {
            use std::fmt::Write;
            let value = (row as f32 * 0.1).sin();
            writeln!(&mut data, "{row},{value},{},,,", -value).unwrap();
        }
        fs::write(&path, data).unwrap();
        let cancel = AtomicBool::new(false);
        let mut view = Viewer::load(&path, &cancel).unwrap();
        view.stacked = true;
        view.visible[1] = false;
        view.display_offsets[0] = 0.25;
        view.cursors = [Some(0.0625), Some(0.125)];
        view.marks.push(MarkedRange {
            span: 0.0625..0.125,
            peak_to_peak: Some((0.5, "V".into())),
        });
        view.add_filter(FilterKind::LowPass);
        view.active = 1;
        view.add_filter(FilterKind::HighPass);
        view.active = 0;

        let original = serde_json::to_string(&view.session_settings()).unwrap();
        let mut reopened = Viewer::load(&path, &cancel).unwrap();
        reopened
            .restore_session(serde_json::from_str(&original).unwrap())
            .unwrap();
        assert_eq!(
            serde_json::to_string(&reopened.session_settings()).unwrap(),
            original
        );

        let ctx = egui::Context::default();
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            let _ = clip_ui::test_support::run_frame(
                &ctx,
                egui::RawInput {
                    time: Some(start.elapsed().as_secs_f64()),
                    ..Default::default()
                },
                |ctx| {
                    ctx.root_ui(|ui| {
                        egui::CentralPanel::default().show(ui, |ui| {
                            reopened
                                .filters
                                .update(ui, &reopened.recording, reopened.active);
                        });
                    });
                },
            );
            let ready: Vec<_> = reopened
                .filtered_channels()
                .filter(|lane| lane.samples.is_some())
                .map(|lane| lane.channel)
                .collect();
            if ready == [0, 1] {
                return;
            }
            thread::sleep(Duration::from_millis(15));
        }
        panic!("filters did not rebuild for both saved channels");
    }
}
