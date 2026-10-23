//! Files previews one explicitly chosen track; playback never advances or repeats.
pub(super) mod preview;
mod selection;
mod spectrum;
use super::ui::music::MusicRange;
use little_clip_core::{BackgroundJobs, JobError, JobTicket};
use rodio::{Decoder, Player, Source};
use std::{fs::File, io::BufReader, path::PathBuf, sync::mpsc, time::Duration};

type AudioDecoder = Decoder<BufReader<File>>;
fn audio_file(path: &std::path::Path) -> Result<File, String> {
    crate::file_source::open_regular_file(path, true).map_err(|e| e.to_string())
}
fn decode_file(path: &std::path::Path) -> Result<AudioDecoder, String> {
    Decoder::try_from(audio_file(path)?).map_err(|e| e.to_string())
}
struct Prepared {
    audio: Box<dyn Source<Item = f32> + Send>,
    duration: f32,
    offset: f32,
}
struct Loading {
    ticket: JobTicket,
    receiver: mpsc::Receiver<Result<Result<Prepared, String>, JobError>>,
}

struct CopyJob {
    ticket: JobTicket,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    receiver: mpsc::Receiver<Result<Result<String, String>, JobError>>,
}
impl Drop for CopyJob {
    fn drop(&mut self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.ticket.cancel();
    }
}
impl Drop for Loading {
    fn drop(&mut self) {
        self.ticket.cancel();
    }
}

#[derive(Default)]
pub(super) struct Music {
    current: Option<PathBuf>,
    player: Option<Player>,
    loading: Option<Loading>,
    pub duration: f32,
    pub error: Option<String>,
    seek_pending: Option<f32>,
    copy_job: Option<CopyJob>,
    notice: Option<String>,
    offset: f32,
    range_playback: bool,
    snapping: Option<selection::Snapping>,
}
impl Music {
    pub fn current(&self) -> Option<&PathBuf> {
        self.current.as_ref()
    }
    pub fn play(&mut self, path: &std::path::Path) {
        self.stop();
        self.current = Some(path.to_path_buf());
        self.load_current();
    }
    pub fn play_range(&mut self, range: MusicRange) {
        self.stop();
        self.current = Some(range.path.clone());
        self.range_playback = true;
        self.load(Some(range));
    }
    fn load_current(&mut self) {
        self.load(None);
    }
    fn load(&mut self, range: Option<MusicRange>) {
        self.player = None;
        self.loading = None;
        self.duration = 0.0;
        self.seek_pending = None;
        self.error = None;
        let Some(path) = self.current.clone() else {
            return;
        };
        let (ticket, receiver) =
            BackgroundJobs::global().submit(Duration::from_secs(30), move || {
                let source = crate::file_source::FileSource::capture(&path)?;
                let decoder = decode_file(&path)?;
                let duration = decoder
                    .total_duration()
                    .map(|d| d.as_secs_f32())
                    .unwrap_or_else(|| {
                        little_clip_core::audio::read_audio_metadata(&path)
                            .map_or(0.0, |m| m.duration_ms as f32 / 1000.0)
                    });
                let prepared = if let Some(range) = range {
                    let range = selection::snap(range)?;
                    if source != range.source {
                        return Err("Audio changed while opening the selection".into());
                    }
                    let start = range.start.min(range.end);
                    let end = range.start.max(range.end);
                    if end <= start {
                        return Err("Select a non-empty audio range.".into());
                    }
                    let mut decoder = decoder;
                    decoder
                        .try_seek(Duration::from_secs_f32(start))
                        .map_err(|e| e.to_string())?;
                    Prepared {
                        audio: Box::new(
                            decoder.take_duration(Duration::from_secs_f32(end - start)),
                        ),
                        duration,
                        offset: start,
                    }
                } else {
                    Prepared {
                        audio: Box::new(decoder),
                        duration,
                        offset: 0.0,
                    }
                };
                source.check()?;
                if crate::file_source::FileSource::capture(&path)? != source {
                    return Err("Audio path changed while opening".into());
                }
                Ok(prepared)
            });
        self.loading = Some(Loading { ticket, receiver });
    }
    pub fn tick(&mut self, ctx: &egui::Context, selection: &mut Option<MusicRange>) {
        self.tick_with_output(ctx, selection, |source| {
            let player = crate::ui_ext::new_shared_audio_player()?;
            player.append(source);
            Some(player)
        });
    }
    /// Uses the existing guarded streaming loader with the caller's output bus.
    pub(super) fn tick_with_output(
        &mut self,
        ctx: &egui::Context,
        selection: &mut Option<MusicRange>,
        mut output: impl FnMut(Box<dyn Source<Item = f32> + Send>) -> Option<Player>,
    ) {
        self.poll_selection(selection);
        if ctx.input(|input| input.pointer.any_released())
            && let Some(range) = selection.as_ref().filter(|range| !range.snapped).cloned()
        {
            self.snap_selection(range);
        }
        if let Some(result) = self
            .copy_job
            .as_ref()
            .and_then(|job| super::poll_job(&job.receiver))
        {
            self.copy_job = None;
            match result {
                Ok(notice) => self.notice = Some(notice),
                Err(error) => self.error = Some(error),
            }
        }
        if let Some(result) = self
            .loading
            .as_ref()
            .and_then(|l| super::poll_job(&l.receiver))
        {
            self.loading = None;
            match result {
                Ok(prepared) => {
                    self.duration = prepared.duration;
                    self.offset = prepared.offset;
                    if let Some(player) = output(prepared.audio) {
                        player.pause();
                        if let Some(time) = self.seek_pending.take() {
                            if let Err(error) = player
                                .try_seek(Duration::from_secs_f32(time.clamp(0.0, self.duration)))
                            {
                                self.error = Some(error.to_string());
                            }
                        }
                        player.play();
                        self.player = Some(player);
                    } else {
                        self.error = Some("No audio output device is available.".into());
                    }
                }
                Err(error) => {
                    self.error = Some(format!("Cannot play this track: {error}"));
                }
            }
        }
        if self.player.as_ref().is_some_and(|p| p.empty()) {
            self.stop();
        }
        if self.loading.is_some()
            || self.copy_job.is_some()
            || self.snapping.is_some()
            || self.playing()
        {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
    pub fn playing(&self) -> bool {
        self.player
            .as_ref()
            .is_some_and(|p| !p.is_paused() && !p.empty())
    }
    pub fn busy(&self) -> bool {
        self.loading.is_some()
    }
    pub fn position(&self) -> f32 {
        self.player
            .as_ref()
            .map_or(self.seek_pending.unwrap_or(0.0), |p| {
                self.offset + p.get_pos().as_secs_f32()
            })
    }
    pub fn seek(&mut self, time: f32) {
        if !time.is_finite() {
            return;
        }
        if self.range_playback {
            self.range_playback = false;
            self.load_current();
        }
        if let Some(player) = &self.player {
            if let Err(error) =
                player.try_seek(Duration::from_secs_f32(time.clamp(0.0, self.duration)))
            {
                self.error = Some(format!("Cannot seek: {error}"));
            }
        } else {
            if self.loading.is_none() {
                self.load_current();
            }
            if self.loading.is_some() {
                self.seek_pending = Some(time.max(0.0));
            }
        }
    }
    pub fn stop(&mut self) {
        self.player = None;
        self.loading = None;
        self.current = None;
        self.duration = 0.0;
        self.seek_pending = None;
        self.offset = 0.0;
        self.range_playback = false;
    }
    pub fn copying(&self) -> bool {
        self.copy_job.is_some()
    }
    pub fn status(&self) -> Option<&str> {
        self.error.as_deref().or(self.notice.as_deref())
    }
    pub fn copy_range(&mut self, range: super::ui::music::MusicRange) {
        if self.copy_job.is_some() {
            return;
        }
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let token = cancelled.clone();
        let (ticket, receiver) =
            BackgroundJobs::global().submit(Duration::from_secs(120), move || {
                let range = selection::snap(range)?;
                crate::audio_preview::copy_file_audio_segment(
                    &range.path,
                    &range.source,
                    range.start,
                    range.end,
                    &token,
                )
            });
        self.copy_job = Some(CopyJob {
            ticket,
            cancelled,
            receiver,
        });
        self.notice = Some("Preparing selected audio for the clipboard…".into());
    }
}
