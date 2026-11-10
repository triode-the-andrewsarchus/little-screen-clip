use super::Document;
use crate::{midi_view::MidiViewerState, ui_ext::StatusToast};
use std::{path::PathBuf, sync::Arc};

#[derive(Clone)]
pub(in crate::file_manager) struct Range {
    pub path: PathBuf,
    pub revision: u64,
    pub document: Arc<Document>,
    pub start: f32,
    pub end: f32,
}

#[derive(Default)]
pub(in crate::file_manager) struct Preview {
    playback: Option<(u64, MidiViewerState)>,
    copying: Option<MidiViewerState>,
    pub selection: Option<Range>,
    pub queued: Option<(PathBuf, u64, Arc<Document>, f32)>,
    playback_notice: StatusToast,
    copy_notice: StatusToast,
}
impl Preview {
    pub fn stop(&mut self) {
        self.playback_notice.clear();
        if let Some((_, state)) = &mut self.playback {
            state.stop();
        }
        self.playback = None;
        self.queued = None;
    }
    pub fn play(
        &mut self,
        path: PathBuf,
        revision: u64,
        document: Arc<Document>,
        start: f32,
        end: Option<f32>,
    ) {
        self.playback_notice.clear();
        if !self.playback.as_ref().is_some_and(|(version, state)| {
            *version == revision && state.path.as_ref() == Some(&path)
        }) {
            self.stop();
            self.playback = Some((
                revision,
                MidiViewerState::from_shared(
                    Arc::clone(&document.data),
                    Some(path),
                    document.data.title.clone(),
                ),
            ));
        }
        let state = &mut self.playback.as_mut().unwrap().1;
        state.stop();
        state.playhead_sec = start.clamp(0.0, document.data.duration_sec);
        state.play_stop_time = end;
        state.play();
    }
    pub fn play_selection(&mut self) -> bool {
        let Some(range) = self
            .selection
            .clone()
            .filter(|range| (range.end - range.start).abs() > 0.001)
        else {
            return false;
        };
        self.play(
            range.path,
            range.revision,
            range.document,
            range.start.min(range.end),
            Some(range.start.max(range.end)),
        );
        true
    }
    pub fn position(&self, path: &std::path::Path, revision: u64) -> Option<f32> {
        self.playback
            .as_ref()
            .filter(|(version, state)| {
                *version == revision
                    && state.path.as_deref() == Some(path)
                    && (state.is_playing || state.pending_play)
            })
            .map(|(_, state)| state.playhead_sec)
    }
    pub fn copy_selection(&mut self, now: f64) {
        if self.copying() {
            return;
        }
        let Some(range) = self
            .selection
            .as_ref()
            .filter(|range| (range.end - range.start).abs() > 0.001)
        else {
            return;
        };
        let mut state = MidiViewerState::from_shared(
            Arc::clone(&range.document.data),
            Some(range.path.clone()),
            range.document.data.title.clone(),
        );
        state.selected_region = Some((range.start, range.end));
        state.copy_selection(now);
        self.copying = Some(state);
        self.copy_notice.clear();
    }
    pub fn copying(&self) -> bool {
        self.copying
            .as_ref()
            .is_some_and(MidiViewerState::export_pending)
    }
    pub fn tick(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|input| input.time);
        if let Some((_, state)) = &mut self.playback {
            state.update_playback();
            let feedback_now = crate::ui_ext::feedback_time();
            if let Some((message, started)) = state
                .playback_status
                .active(feedback_now)
                .map(|(message, started)| (message.clone(), now - (feedback_now - started)))
            {
                self.playback_notice.set(message, started);
                state.playback_status.clear();
            }
            if state.is_playing || state.is_synthesizing {
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
        }
        if let Some(state) = &mut self.copying {
            state.poll_export(now, ctx);
            if !state.export_pending() {
                if let Some((message, _)) = state.status_toast.active(now) {
                    self.copy_notice.set(message.clone(), now);
                }
                self.copying = None;
            }
        }
    }

    pub fn render_feedback(
        &mut self,
        ui: &mut egui::Ui,
        palette: &little_clip_core::palette::AppPalette,
    ) {
        let now = ui.input(|input| input.time);
        if self.copying()
            || self
                .playback
                .as_ref()
                .is_some_and(|(_, state)| state.is_synthesizing)
        {
            let message = if self.copying() {
                "Preparing selected MIDI for the clipboard…"
            } else {
                "Synthesizing MIDI audio…"
            };
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(message);
            });
        }
        self.copy_notice.render(ui, now, palette);
        self.playback_notice.render(ui, now, palette);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_ext::StatusMessage;

    fn state() -> MidiViewerState {
        MidiViewerState::from_shared(
            Arc::new(little_clip_core::presentation::midi::MidiData {
                title: "Feedback fixture".into(),
                track_count: 1,
                track_names: vec!["Track".into()],
                total_notes: 0,
                duration_sec: 1.0,
                bpm: 120.0,
                min_key: 60,
                max_key: 60,
                notes: Vec::new(),
            }),
            Some(PathBuf::from("fixture.mid")),
            "Feedback fixture".into(),
        )
    }

    #[test]
    fn forwarded_playback_feedback_keeps_its_age_across_clock_domains() {
        let ctx = egui::Context::default();
        let mut preview = Preview::default();
        let mut playback = state();
        playback
            .playback_status
            .set("playback information", crate::ui_ext::feedback_time() - 1.0);
        preview.playback = Some((1, playback));
        let _ = ctx.run(
            egui::RawInput {
                time: Some(100.0),
                ..Default::default()
            },
            |ctx| preview.tick(ctx),
        );
        assert_eq!(
            preview.playback_notice.message(102.5),
            Some("playback information")
        );
        assert_eq!(preview.playback_notice.message(103.5), None);
    }

    #[test]
    fn copy_failure_survives_playback_retirement_and_cached_retry() {
        let ctx = egui::Context::default();
        let mut preview = Preview::default();
        let mut copy = state();
        copy.status_toast
            .set(StatusMessage::error("copy failure"), 0.0);
        preview.copying = Some(copy);
        let _ = ctx.run(egui::RawInput::default(), |ctx| preview.tick(ctx));
        preview
            .playback_notice
            .set(StatusMessage::error("old playback failure"), 0.0);
        preview.stop();
        assert_eq!(preview.copy_notice.message(100.0), Some("copy failure"));
        assert!(!preview.playback_notice.is_active(100.0));

        let mut cached = state();
        cached.wav_cache = Some(Arc::from([]));
        let document = Arc::new(Document {
            data: Arc::clone(&cached.data),
            low: 60,
            high: 60,
            cropped: 0,
            groups: Vec::new(),
        });
        preview.playback = Some((1, cached));
        preview
            .playback_notice
            .set(StatusMessage::error("old playback failure"), 0.0);
        // EOF exercises reuse/reset without opening a real audio device.
        preview.play(PathBuf::from("fixture.mid"), 1, document, 1.0, None);
        assert!(!preview.playback_notice.is_active(100.0));
        assert_eq!(preview.copy_notice.message(100.0), Some("copy failure"));
    }

    #[test]
    fn strip_feedback_schedules_expiry_and_keeps_copy_errors_visible() {
        let ctx = egui::Context::default();
        let palette = little_clip_core::palette::AppPalette::default();
        let mut preview = Preview::default();
        preview.playback_notice.set("temporary feedback", 0.0);
        let mut output = egui::FullOutput::default();
        for _ in 0..8 {
            output = ctx.run(
                egui::RawInput {
                    time: Some(0.25),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default()
                        .show(ctx, |ui| preview.render_feedback(ui, &palette));
                },
            );
        }
        let delay = output.viewport_output[&egui::ViewportId::ROOT]
            .repaint_delay
            .as_secs_f64();
        assert!(delay > 3.6 && delay <= 3.75, "{delay}");
        preview.playback_notice.clear();
        preview
            .copy_notice
            .set(StatusMessage::error("persistent copy failure"), 0.0);
        let output = ctx.run(
            egui::RawInput {
                time: Some(100.0),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| preview.render_feedback(ui, &palette));
            },
        );
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Text(text) if text.galley.text() == "persistent copy failure")));
    }
}
