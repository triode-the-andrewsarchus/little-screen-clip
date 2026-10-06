//! Files owns one streaming player and an explicit queue across folder navigation.
pub(super) mod preview;
mod queue;
use little_clip_core::{BackgroundJobs, JobError, JobTicket};
use queue::Queue;
use rodio::{Decoder, Player, Source};
use std::os::unix::fs::OpenOptionsExt;
use std::{fs::File, io::BufReader, path::PathBuf, sync::mpsc, time::Duration};

type AudioDecoder = Decoder<BufReader<File>>;
fn decode_file(path: &std::path::Path) -> Result<AudioDecoder, String> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| e.to_string())?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Audio source is not a regular file".into());
    }
    Decoder::try_from(file).map_err(|e| e.to_string())
}
struct Loading {
    ticket: JobTicket,
    receiver: mpsc::Receiver<Result<Result<(AudioDecoder, f32), String>, JobError>>,
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
    pub queue: Queue,
    player: Option<Player>,
    loading: Option<Loading>,
    pub duration: f32,
    pub error: Option<String>,
    seek_pending: Option<f32>,
    paused: bool,
    copy_job: Option<CopyJob>,
    notice: Option<String>,
}
impl Music {
    pub fn play_folder(&mut self, paths: Vec<PathBuf>, path: &std::path::Path) {
        if self.queue.replace(paths, path) {
            self.load_current();
        }
    }
    pub fn load_current(&mut self) {
        self.player = None;
        self.loading = None;
        self.duration = 0.0;
        self.seek_pending = None;
        self.error = None;
        self.paused = false;
        let Some(path) = self.queue.current().cloned() else {
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
                source.check()?;
                if crate::file_source::FileSource::capture(&path)? != source {
                    return Err("Audio path changed while opening".into());
                }
                Ok((decoder, duration))
            });
        self.loading = Some(Loading { ticket, receiver });
    }
    pub fn tick(&mut self, ctx: &egui::Context) {
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
                Ok((decoder, duration)) => {
                    self.duration = duration;
                    if let Some(player) = crate::ui_ext::new_shared_audio_player() {
                        player.pause();
                        player.append(decoder);
                        if let Some(time) = self.seek_pending.take() {
                            if let Err(error) =
                                player.try_seek(Duration::from_secs_f32(time.clamp(0.0, duration)))
                            {
                                self.error = Some(error.to_string());
                            }
                        }
                        if !self.paused {
                            player.play();
                        }
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
            self.player = None;
            if self.queue.advance(true, false) {
                self.load_current();
            } else {
                self.seek_pending = Some(self.duration);
            }
        }
        if self.loading.is_some() || self.copy_job.is_some() || self.playing() {
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
                p.get_pos().as_secs_f32()
            })
    }
    pub fn pause(&mut self) {
        self.paused = true;
        if let Some(player) = &self.player {
            player.pause();
        }
    }
    pub fn toggle(&mut self) {
        if self.loading.is_some() {
            self.paused = !self.paused;
        } else if let Some(player) = &self.player {
            if player.is_paused() {
                player.play();
                self.paused = false;
            } else {
                self.pause();
            }
        } else {
            self.load_current();
        }
    }
    pub fn seek(&mut self, time: f32) {
        if !time.is_finite() {
            return;
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
    pub fn step(&mut self, forward: bool) {
        if self.queue.advance(forward, true) {
            self.load_current();
        }
    }
    pub fn clear(&mut self) {
        self.player = None;
        self.loading = None;
        self.queue = Queue::default();
        self.duration = 0.0;
        self.seek_pending = None;
        self.paused = false;
        self.error = None;
        self.notice = None;
    }
    pub fn copying(&self) -> bool {
        self.copy_job.is_some()
    }
    pub fn active(&self) -> bool {
        !self.queue.paths.is_empty()
            || self.copying()
            || self.notice.is_some()
            || self.error.is_some()
    }
    pub fn copy_range(&mut self, range: super::ui::music::MusicRange) {
        if self.copy_job.is_some() {
            return;
        }
        let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let token = cancelled.clone();
        let (ticket, receiver) =
            BackgroundJobs::global().submit(Duration::from_secs(120), move || {
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
    fn playback_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Previous track").clicked() {
                self.step(false);
            }
            if ui
                .button(if self.playing() {
                    "Pause music"
                } else {
                    "Play music"
                })
                .clicked()
            {
                self.toggle();
            }
            if ui.button("Next track").clicked() {
                self.step(true);
            }
            if ui
                .button(format!("Repeat: {}", self.queue.repeat.label()))
                .clicked()
            {
                self.queue.repeat = self.queue.repeat.next();
            }
            if ui
                .button(format!(
                    "Shuffle: {}",
                    if self.queue.shuffled { "on" } else { "off" }
                ))
                .clicked()
            {
                self.queue.toggle_shuffle();
            }
            if ui
                .add_enabled(!self.copying(), egui::Button::new("Clear queue"))
                .clicked()
            {
                self.clear();
            }
        });
    }
    fn queue_ui(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new(format!("Queue ({} tracks)", self.queue.paths.len())).show(
            ui,
            |ui| {
                let mut selected = None;
                egui::ScrollArea::vertical().max_height(120.0).show_rows(
                    ui,
                    24.0,
                    self.queue.paths.len(),
                    |ui, rows| {
                        for index in rows {
                            if ui
                                .selectable_label(
                                    self.queue.current() == Some(&self.queue.paths[index]),
                                    super::display_path(&self.queue.paths[index]),
                                )
                                .clicked()
                            {
                                selected = Some(index);
                            }
                        }
                    },
                );
                if let Some(index) = selected {
                    self.queue.select(index);
                    self.load_current();
                }
            },
        );
    }
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.playback_controls(ui);
        if let Some(path) = self.queue.current() {
            let path = super::display_path(path);
            ui.add(egui::Label::new(&path).truncate(true))
                .on_hover_text(path);
        }
        let mut position = self.position();
        if self.duration > 0.0
            && ui
                .add(
                    egui::Slider::new(&mut position, 0.0..=self.duration)
                        .suffix(" s")
                        .text("Seek"),
                )
                .changed()
        {
            self.seek(position);
        }
        if self.busy() {
            ui.spinner();
            ui.label("Preparing track…");
        }
        if let Some(error) = &self.error {
            ui.label(error);
        }
        if let Some(notice) = &self.notice {
            ui.label(notice);
        }
        self.queue_ui(ui);
    }
}
