//! One focused instrument receives controller notes, with bounded voices and PCM cache.
use super::{Task, audition, surface::Strike};
use rodio::Source;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    On(u8, u8, u8),
    Off(u8, u8),
    Sustain(u8, bool),
    Reset(u8),
}
fn event(bytes: &[u8]) -> Option<Event> {
    let &[status, key, value] = bytes else {
        return None;
    };
    if key > 127 || value > 127 {
        return None;
    }
    let channel = status & 15;
    match status & 0xf0 {
        0x90 if value > 0 => Some(Event::On(channel, key, value)),
        0x80 | 0x90 => Some(Event::Off(channel, key)),
        0xb0 if key == 64 => Some(Event::Sustain(channel, value >= 64)),
        0xb0 if matches!(key, 120 | 123) => Some(Event::Reset(channel)),
        _ => None,
    }
}
struct Input {
    _lifetime: mpsc::Sender<()>,
    events: mpsc::Receiver<Event>,
    overflow: Arc<AtomicBool>,
    status: Arc<Mutex<Option<String>>>,
}
impl Input {
    fn new(ctx: egui::Context) -> Result<Self, String> {
        let (lifetime, stop) = mpsc::channel();
        let (send, events) = mpsc::sync_channel(256);
        let overflow = Arc::new(AtomicBool::new(false));
        let lost = overflow.clone();
        let status = Arc::new(Mutex::new(None));
        let report = status.clone();
        std::thread::Builder::new()
            .name("multisample-midi".into())
            .spawn(move || {
                let mut connections = Default::default();
                loop {
                    let sender = send.clone();
                    let lost_callback = lost.clone();
                    let context = ctx.clone();
                    let result = super::super::midi_input::refresh(
                        &mut connections,
                        "Little Clip instruments",
                        move |bytes| {
                            if let Some(event) = event(bytes) {
                                if sender.try_send(event).is_err() {
                                    lost_callback.store(true, Ordering::Release);
                                }
                                context.request_repaint();
                            }
                        },
                    );
                    if let Ok(mut status) = report.lock() {
                        *status = result.as_ref().err().cloned();
                    }
                    if result != Ok(false) {
                        lost.store(true, Ordering::Release);
                        ctx.request_repaint();
                    }
                    if !matches!(
                        stop.recv_timeout(Duration::from_secs(2)),
                        Err(mpsc::RecvTimeoutError::Timeout)
                    ) {
                        break;
                    }
                }
                // Port teardown joins callback threads here, never on the UI thread.
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            _lifetime: lifetime,
            events,
            overflow,
            status,
        })
    }
}
struct Voice {
    strike: Strike,
    region: usize,
    held: bool,
    task: Option<Task<Result<audition::Audio, String>>>,
    player: Option<rodio::Player>,
}
#[derive(Default)]
pub(super) struct Performance {
    input: Option<Input>,
    voices: BTreeMap<(u8, u8), Voice>,
    sustain: [bool; 16],
    cache: BTreeMap<usize, Arc<audition::Audio>>,
    error: Option<String>,
    pub latest: Option<(Strike, usize)>,
}
impl Performance {
    pub fn disconnect(&mut self) {
        self.input = None;
        self.stop();
    }
    pub fn error(&self) -> Option<String> {
        self.input
            .as_ref()
            .and_then(|input| input.status.try_lock().ok().and_then(|s| s.clone()))
            .or_else(|| self.error.clone())
    }
    pub fn stop(&mut self) {
        if let Some(input) = &self.input {
            for _ in input.events.try_iter() {}
        }
        self.voices.clear();
        self.latest = None;
        self.sustain.fill(false);
    }
    pub fn keys(&self) -> Vec<u8> {
        self.voices
            .values()
            .filter(|v| v.player.is_some())
            .map(|v| v.strike.key)
            .collect()
    }
    pub fn active(&self) -> bool {
        !self.voices.is_empty()
    }
    pub fn poll(
        &mut self,
        ctx: &egui::Context,
        document: &Arc<little_clip_core::multisample::Document>,
        instrument: usize,
        base: &std::path::Path,
        archive: Option<&super::archive::Archive>,
    ) {
        if self.input.is_none() {
            match Input::new(ctx.clone()) {
                Ok(input) => self.input = Some(input),
                Err(e) => {
                    self.error = Some(e);
                    return;
                }
            }
        }
        let input = self.input.as_ref().unwrap();
        let overflow = input.overflow.swap(false, Ordering::AcqRel);
        let events: Vec<_> = input.events.try_iter().take(256).collect();
        if overflow {
            self.stop();
        } else {
            for event in events {
                if let Event::On(channel, key, velocity) = event {
                    self.note_on(
                        channel,
                        Strike { key, velocity },
                        document,
                        instrument,
                        base,
                        archive,
                    );
                } else {
                    self.release(event);
                }
            }
        }
        let Some(preset) = document.instruments.get(instrument) else {
            return;
        };
        for voice in self.voices.values_mut() {
            if let Some(result) = voice.task.as_ref().and_then(Task::poll) {
                voice.task = None;
                match result.map_err(|e| e.to_string()).and_then(|r| r) {
                    Ok(audio) => {
                        // Bounded decoded PCM, shared with active voices; no copies on note-on.
                        let size = audio
                            .source
                            .total_duration()
                            .map_or(0.0, |d| d.as_secs_f32())
                            * audio.source.sample_rate().get() as f32
                            * audio.source.channels().get() as f32;
                        if self.cache.len() >= 8 {
                            self.cache.clear();
                        }
                        let audio = Arc::new(audio);
                        if size <= 2_000_000.0 {
                            self.cache.insert(voice.region, audio.clone());
                        }
                        start_voice(voice, &audio, &preset.regions[voice.region]);
                    }
                    Err(error) => self.error = Some(error),
                }
            }
        }
        self.voices
            .retain(|_, v| v.task.is_some() || v.player.as_ref().is_some_and(|p| !p.empty()));
        if self.active() {
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
    fn note_on(
        &mut self,
        channel: u8,
        strike: Strike,
        document: &Arc<little_clip_core::multisample::Document>,
        instrument: usize,
        base: &std::path::Path,
        archive: Option<&super::archive::Archive>,
    ) {
        self.error = None;
        self.voices.remove(&(channel, strike.key));
        let Some(preset) = document.instruments.get(instrument) else {
            return;
        };
        let Some(index) = super::surface::choose(&preset.regions, strike, None) else {
            return;
        };
        self.latest = Some((strike, index));
        if self.voices.len() >= 16 {
            self.voices.pop_first();
        }
        let mut voice = Voice {
            strike,
            region: index,
            held: true,
            task: None,
            player: None,
        };
        if let Some(audio) = self.cache.get(&index) {
            start_voice(&mut voice, audio, &preset.regions[index]);
        } else {
            let document = document.clone();
            let region = preset.regions[index].clone();
            let base = base.to_owned();
            let archive = archive.cloned();
            voice.task = Some(Task::start(
                little_clip_core::BackgroundJobs::global(),
                Duration::from_secs(35),
                move |token| audition::prepare(&document, &region, &base, archive.as_ref(), token),
            ));
        }
        self.voices.insert((channel, strike.key), voice);
    }
    fn release(&mut self, event: Event) {
        match event {
            Event::Off(channel, key) => {
                if self.sustain[channel as usize] {
                    if let Some(v) = self.voices.get_mut(&(channel, key)) {
                        v.held = false;
                    }
                } else {
                    self.voices.remove(&(channel, key));
                }
            }
            Event::Sustain(channel, down) => {
                self.sustain[channel as usize] = down;
                if !down {
                    self.voices.retain(|&(ch, _), v| ch != channel || v.held);
                }
            }
            Event::Reset(channel) => {
                self.sustain[channel as usize] = false;
                self.voices.retain(|&(ch, _), _| ch != channel);
            }
            Event::On(..) => {}
        }
    }
}
fn start_voice(
    voice: &mut Voice,
    audio: &audition::Audio,
    region: &little_clip_core::multisample::Region,
) {
    if let Some(player) = crate::ui_ext::new_shared_audio_player() {
        let speed = 2.0_f32.powf((f32::from(voice.strike.key) - f32::from(region.root)) / 12.0);
        player.append(
            audio
                .source
                .clone()
                .speed(speed)
                .amplify(audition::velocity_gain(voice.strike.velocity) * 0.25)
                .take_duration(Duration::from_secs(10)),
        );
        voice.player = Some(player);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn releasing_pending_notes_cancels_decoding_and_disconnect_clears_sustain() {
        let mut performance = Performance::default();
        let task = Task::start(
            little_clip_core::BackgroundJobs::global(),
            Duration::from_secs(1),
            |_| Err("owned pending fixture".into()),
        );
        let token = task.token.clone();
        performance.voices.insert(
            (0, 60),
            Voice {
                strike: Strike {
                    key: 60,
                    velocity: 100,
                },
                region: 0,
                held: true,
                task: Some(task),
                player: None,
            },
        );
        performance.release(Event::Off(0, 60));
        assert!(token.load(Ordering::Relaxed));
        assert!(performance.voices.is_empty());
        performance.sustain[2] = true;
        performance.disconnect();
        assert!(performance.sustain.iter().all(|&s| !s));
    }
    #[test]
    fn midi_zero_velocity_releases_and_sustain_is_channel_scoped() {
        assert_eq!(event(&[0x93, 60, 0]), Some(Event::Off(3, 60)));
        assert_eq!(event(&[0xb2, 64, 127]), Some(Event::Sustain(2, true)));
        assert_eq!(event(&[0x90, 60, 255]), None);
        let mut p = Performance::default();
        for channel in [0, 1] {
            p.voices.insert(
                (channel, 60),
                Voice {
                    strike: Strike {
                        key: 60,
                        velocity: 96,
                    },
                    region: 0,
                    held: true,
                    task: None,
                    player: None,
                },
            );
        }
        p.release(Event::Sustain(0, true));
        p.release(Event::Off(0, 60));
        p.release(Event::Off(1, 60));
        assert_eq!(p.voices.len(), 1);
        p.release(Event::Sustain(0, false));
        assert!(p.voices.is_empty());
    }
}
