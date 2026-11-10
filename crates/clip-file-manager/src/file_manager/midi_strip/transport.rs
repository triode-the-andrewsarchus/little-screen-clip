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
    pub notice: StatusToast,
}
impl Preview {
    pub fn stop(&mut self) {
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
        self.notice
            .set("Preparing selected MIDI for the clipboard…", now);
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
            let epoch = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0.0, |time| time.as_secs_f64());
            if let Some(message) = state.status_toast.message(epoch).map(str::to_owned) {
                self.notice.set(message, now);
                state.status_toast.clear();
            }
            if state.is_playing || state.is_synthesizing {
                ctx.request_repaint_after(std::time::Duration::from_millis(16));
            }
        }
        if let Some(state) = &mut self.copying {
            state.poll_export(now, ctx);
            if !state.export_pending() {
                if let Some(message) = state.status_toast.message(now) {
                    self.notice.set(message, now);
                }
                self.copying = None;
            }
        }
    }
}
