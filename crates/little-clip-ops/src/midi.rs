//! MIDI file parsing, note event extraction, and software synthesis.
//!
//! Parses Standard MIDI Files (`.mid`), extracts polyphonic note sequences with timing,
//! pitch, velocity, and program instrumentation, and provides a built-in multi-timbral
//! software synthesizer generating 44.1kHz 16-bit PCM WAV audio.

use midly::{MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};
use std::collections::HashMap;
use std::path::Path;

/// An individual parsed MIDI note event.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MidiNote {
    /// Zero-based track index where the note was defined.
    pub track: usize,
    /// MIDI channel (0..15; channel 9 is GM percussion).
    pub channel: u8,
    /// General MIDI program / instrument patch (0..127).
    #[serde(default)]
    pub program: u8,
    /// MIDI note key number (0..127; 60 = Middle C / C4).
    pub key: u8,
    /// Note strike velocity (1..127).
    pub velocity: u8,
    /// Absolute start timestamp in seconds.
    pub start_sec: f32,
    /// Note duration in seconds.
    pub duration_sec: f32,
}

impl MidiNote {
    /// Returns the pitch class name without octave (e.g. "C", "F#").
    pub fn note_name(&self) -> &'static str {
        const NOTE_NAMES: [&str; 12] = [
            "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
        ];
        NOTE_NAMES[(self.key % 12) as usize]
    }

    /// Returns the scientific octave number (e.g. 4 for Middle C).
    pub fn octave(&self) -> i32 {
        (self.key as i32 / 12) - 1
    }

    /// Returns the full scientific note name with octave (e.g. "C4", "A#3").
    pub fn full_name(&self) -> String {
        format!("{}{}", self.note_name(), self.octave())
    }

    /// Computes the fundamental frequency in Hertz (based on A4 = 440 Hz equal temperament).
    pub fn frequency(&self) -> f32 {
        440.0 * 2.0_f32.powf((self.key as f32 - 69.0) / 12.0)
    }
}

/// Parsed MIDI score containing metadata, track names, and note sequences.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MidiData {
    /// Score title or filename.
    pub title: String,
    /// Total number of tracks.
    pub track_count: usize,
    /// Names of individual tracks if declared in MIDI meta events.
    #[serde(default)]
    pub track_names: Vec<String>,
    /// Total number of note events across all tracks.
    pub total_notes: usize,
    /// Estimated total duration of the score in seconds.
    pub duration_sec: f32,
    /// Initial tempo in beats per minute (BPM).
    pub bpm: f32,
    /// Lowest MIDI note key present in the score.
    pub min_key: u8,
    /// Highest MIDI note key present in the score.
    pub max_key: u8,
    /// Chronologically ordered list of all parsed notes.
    pub notes: Vec<MidiNote>,
}

/// Internal instrument class used for software sound synthesis.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum InstrumentType {
    Drums,
    Bass,
    Piano,
    StringsPad,
    Organ,
    GuitarPluck,
    LeadBrass,
}

impl MidiData {
    /// Reads and parses a MIDI file from disk.
    pub fn parse_file<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let bytes =
            std::fs::read(path.as_ref()).map_err(|e| format!("Failed to read MIDI file: {}", e))?;
        let title = path
            .as_ref()
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("MIDI")
            .to_string();
        Self::parse_bytes(&bytes, title)
    }

    /// Parses raw Standard MIDI File (`.mid`) byte payload.
    pub fn parse_bytes(bytes: &[u8], default_title: String) -> Result<Self, String> {
        let smf = Smf::parse(bytes).map_err(|e| format!("Failed to parse MIDI: {}", e))?;

        let ticks_per_beat = match smf.header.timing {
            Timing::Metrical(tpb) => tpb.as_int() as f64,
            Timing::Timecode(fps, sub) => (fps.as_f32() * sub as f32) as f64,
        };

        let mut initial_tempo_us_per_beat: f64 = 500_000.0; // 120 BPM default
        let mut title = default_title;

        let mut track_names: Vec<String> = Vec::with_capacity(smf.tracks.len());

        // First pass: look for tempo & track names in meta events
        for (t_idx, track) in smf.tracks.iter().enumerate() {
            let mut t_name = String::new();
            for event in track {
                if let TrackEventKind::Meta(meta) = event.kind {
                    match meta {
                        MetaMessage::Tempo(us) => {
                            initial_tempo_us_per_beat = us.as_int() as f64;
                        }
                        MetaMessage::TrackName(name_bytes) => {
                            if let Ok(name_str) = std::str::from_utf8(name_bytes) {
                                let trimmed = name_str.trim();
                                if !trimmed.is_empty() {
                                    if t_name.is_empty() {
                                        t_name = trimmed.to_string();
                                    }
                                    if title == "MIDI" {
                                        title = trimmed.to_string();
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            if t_name.is_empty() {
                t_name = format!("Track {}", t_idx + 1);
            }
            track_names.push(t_name);
        }

        let bpm = (60_000_000.0 / initial_tempo_us_per_beat) as f32;
        let sec_per_tick = (initial_tempo_us_per_beat / 1_000_000.0) / ticks_per_beat;

        let mut all_notes: Vec<MidiNote> = Vec::new();
        let mut max_end_sec: f32 = 0.0;
        let mut min_k: u8 = 127;
        let mut max_k: u8 = 0;

        for (t_idx, track) in smf.tracks.iter().enumerate() {
            let mut current_tick: u64 = 0;
            // active note map: (channel, key) -> (start_sec, velocity, program)
            let mut active_notes: HashMap<(u8, u8), (f32, u8, u8)> = HashMap::new();
            let mut channel_programs: HashMap<u8, u8> = HashMap::new();

            for event in track {
                current_tick += event.delta.as_int() as u64;
                let current_sec = (current_tick as f64 * sec_per_tick) as f32;

                if let TrackEventKind::Midi { channel, message } = event.kind {
                    let ch = channel.as_int();
                    match message {
                        MidiMessage::ProgramChange { program } => {
                            channel_programs.insert(ch, program.as_int());
                        }
                        MidiMessage::NoteOn { key, vel } => {
                            let k = key.as_int();
                            let v = vel.as_int();
                            let prog = *channel_programs.get(&ch).unwrap_or(&0);
                            if v > 0 {
                                // Note on
                                active_notes.insert((ch, k), (current_sec, v, prog));
                            } else if let Some((start_sec, old_vel, note_prog)) =
                                active_notes.remove(&(ch, k))
                            {
                                // Note on with 0 velocity is Note off
                                let dur = (current_sec - start_sec).max(0.05);
                                if k < min_k {
                                    min_k = k;
                                }
                                if k > max_k {
                                    max_k = k;
                                }
                                if start_sec + dur > max_end_sec {
                                    max_end_sec = start_sec + dur;
                                }
                                all_notes.push(MidiNote {
                                    track: t_idx,
                                    channel: ch,
                                    program: note_prog,
                                    key: k,
                                    velocity: old_vel,
                                    start_sec,
                                    duration_sec: dur,
                                });
                            }
                        }
                        MidiMessage::NoteOff { key, .. } => {
                            let k = key.as_int();
                            if let Some((start_sec, old_vel, note_prog)) =
                                active_notes.remove(&(ch, k))
                            {
                                let dur = (current_sec - start_sec).max(0.05);
                                if k < min_k {
                                    min_k = k;
                                }
                                if k > max_k {
                                    max_k = k;
                                }
                                if start_sec + dur > max_end_sec {
                                    max_end_sec = start_sec + dur;
                                }
                                all_notes.push(MidiNote {
                                    track: t_idx,
                                    channel: ch,
                                    program: note_prog,
                                    key: k,
                                    velocity: old_vel,
                                    start_sec,
                                    duration_sec: dur,
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }

            // Flush remaining active notes
            let end_sec = (current_tick as f64 * sec_per_tick) as f32;
            for ((ch, k), (start_sec, v, note_prog)) in active_notes {
                let dur = (end_sec - start_sec).max(0.1);
                if k < min_k {
                    min_k = k;
                }
                if k > max_k {
                    max_k = k;
                }
                if start_sec + dur > max_end_sec {
                    max_end_sec = start_sec + dur;
                }
                all_notes.push(MidiNote {
                    track: t_idx,
                    channel: ch,
                    program: note_prog,
                    key: k,
                    velocity: v,
                    start_sec,
                    duration_sec: dur,
                });
            }
        }

        all_notes.sort_by(|a, b| {
            a.start_sec
                .partial_cmp(&b.start_sec)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if min_k > max_k {
            min_k = 48; // C3
            max_k = 72; // C5
        }

        // If Type 0 (single track in SMF) but notes span multiple distinct channels, split into virtual tracks per channel
        if smf.tracks.len() <= 1 {
            let mut unique_channels: Vec<u8> = all_notes.iter().map(|n| n.channel).collect();
            unique_channels.sort_unstable();
            unique_channels.dedup();
            if unique_channels.len() > 1 {
                let mut new_track_names = Vec::new();
                for &ch in &unique_channels {
                    let first_note = all_notes.iter().find(|n| n.channel == ch);
                    let prog = first_note.map(|n| n.program).unwrap_or(0);
                    let name = if ch == 9 {
                        "Drums (Ch 10)".to_string()
                    } else {
                        format!("Channel {} (Patch {})", ch + 1, prog)
                    };
                    new_track_names.push(name);
                }
                for note in &mut all_notes {
                    if let Some(pos) = unique_channels.iter().position(|&c| c == note.channel) {
                        note.track = pos;
                    }
                }
                track_names = new_track_names;
            }
        }

        let track_count = track_names.len().max(smf.tracks.len());

        Ok(Self {
            title,
            track_count,
            track_names,
            total_notes: all_notes.len(),
            duration_sec: max_end_sec.max(1.0),
            bpm: if bpm > 20.0 && bpm < 400.0 {
                bpm
            } else {
                120.0
            },
            min_key: min_k,
            max_key: max_k,
            notes: all_notes,
        })
    }

    /// Heuristically classifies note instrument type based on track channel, program patch, and track name.
    fn resolve_instrument(&self, note: &MidiNote) -> InstrumentType {
        // Channel 9 is standard General MIDI Percussion (0-indexed)
        if note.channel == 9 {
            return InstrumentType::Drums;
        }

        let track_name = self
            .track_names
            .get(note.track)
            .map(|s| s.as_str())
            .unwrap_or("");
        let lower = track_name.to_lowercase();
        if lower.contains("drum")
            || lower.contains("perc")
            || lower.contains("kit")
            || lower.contains("beat")
        {
            return InstrumentType::Drums;
        }
        if note.program >= 112 && note.program <= 119 {
            return InstrumentType::Drums;
        }
        if (note.program >= 32 && note.program <= 39) || lower.contains("bass") {
            return InstrumentType::Bass;
        }
        if (note.program >= 16 && note.program <= 23) || lower.contains("organ") {
            return InstrumentType::Organ;
        }
        if (note.program >= 40 && note.program <= 55)
            || (note.program >= 88 && note.program <= 95)
            || lower.contains("string")
            || lower.contains("pad")
            || lower.contains("viol")
            || lower.contains("cello")
            || lower.contains("orchestra")
        {
            return InstrumentType::StringsPad;
        }
        if (note.program >= 56 && note.program <= 87)
            || lower.contains("lead")
            || lower.contains("brass")
            || lower.contains("horn")
            || lower.contains("synth")
            || lower.contains("solo")
        {
            return InstrumentType::LeadBrass;
        }
        if (note.program >= 24 && note.program <= 31)
            || lower.contains("guitar")
            || lower.contains("pluck")
        {
            return InstrumentType::GuitarPluck;
        }

        // Multi-track fallback when file doesn't set explicit programs
        if self.track_count > 1 && note.program == 0 {
            match note.track % 6 {
                0 => InstrumentType::Piano,
                1 => InstrumentType::Bass,
                2 => InstrumentType::StringsPad,
                3 => InstrumentType::LeadBrass,
                4 => InstrumentType::GuitarPluck,
                _ => InstrumentType::Organ,
            }
        } else {
            InstrumentType::Piano
        }
    }

    /// Extracts a time slice of MIDI notes between `start_sec` and `end_sec`.
    ///
    /// If `solo_track` is specified, only notes from that track are retained and
    /// remapped to a single track. Note timings are re-anchored to `start_sec` = 0.0
    /// and clamped to the slice boundary.
    pub fn slice(&self, start_sec: f32, end_sec: f32, solo_track: Option<usize>) -> Self {
        let s = start_sec.min(end_sec).max(0.0);
        let e = start_sec.max(end_sec);
        let duration = (e - s).max(0.05);

        let mut sliced_notes = Vec::new();
        let mut min_k: u8 = 127;
        let mut max_k: u8 = 0;

        for note in &self.notes {
            if solo_track.is_some_and(|st| note.track != st) {
                continue;
            }

            let note_end = note.start_sec + note.duration_sec;
            if note_end <= s || note.start_sec >= e {
                continue;
            }

            let new_start = (note.start_sec - s).max(0.0);
            let new_end = (note_end - s).min(duration);
            let new_dur = (new_end - new_start).max(0.02);

            let new_track = if solo_track.is_some() { 0 } else { note.track };

            if note.key < min_k {
                min_k = note.key;
            }
            if note.key > max_k {
                max_k = note.key;
            }

            sliced_notes.push(MidiNote {
                track: new_track,
                channel: note.channel,
                program: note.program,
                key: note.key,
                velocity: note.velocity,
                start_sec: new_start,
                duration_sec: new_dur,
            });
        }

        sliced_notes.sort_by(|a, b| {
            a.start_sec
                .partial_cmp(&b.start_sec)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if min_k > max_k {
            min_k = self.min_key;
            max_k = self.max_key;
        }

        let (track_count, track_names, title) = if let Some(st) = solo_track {
            let tname = self
                .track_names
                .get(st)
                .cloned()
                .unwrap_or_else(|| format!("Track {}", st + 1));
            (
                1,
                vec![tname.clone()],
                format!("{}_{}_slice", self.title, tname),
            )
        } else {
            (
                self.track_count,
                self.track_names.clone(),
                format!("{}_slice", self.title),
            )
        };

        Self {
            title,
            track_count,
            track_names,
            total_notes: sliced_notes.len(),
            duration_sec: duration,
            bpm: self.bpm,
            min_key: min_k,
            max_key: max_k,
            notes: sliced_notes,
        }
    }

    /// Serializes the score into a Standard MIDI File (`.mid`) byte vector.
    pub fn to_midi_bytes(&self) -> Result<Vec<u8>, String> {
        use midly::num::{u4, u7, u15, u24, u28};
        use midly::{
            Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind,
        };

        let ticks_per_beat = 480u16;
        let header = if self.track_count <= 1 {
            Header::new(
                Format::SingleTrack,
                Timing::Metrical(u15::new(ticks_per_beat)),
            )
        } else {
            Header::new(Format::Parallel, Timing::Metrical(u15::new(ticks_per_beat)))
        };

        let tempo_bpm = if self.bpm > 20.0 && self.bpm < 400.0 {
            self.bpm
        } else {
            120.0
        };
        let tempo_us = (60_000_000.0 / tempo_bpm as f64).round() as u32;
        let ticks_per_sec = (ticks_per_beat as f64 * tempo_bpm as f64) / 60.0;

        let mut smf = Smf::new(header);

        #[derive(Clone, Copy)]
        enum NoteEventType {
            ProgramChange(u8),
            NoteOff { key: u8, vel: u8 },
            NoteOn { key: u8, vel: u8 },
        }

        struct ScheduledEvent {
            tick: u64,
            channel: u8,
            priority: u8, // 0: ProgramChange, 1: NoteOff, 2: NoteOn
            kind: NoteEventType,
        }

        let num_tracks = self.track_count.max(1);
        for t_idx in 0..num_tracks {
            let mut track_events: Vec<TrackEvent> = Vec::new();

            // Track name
            if let Some(name) = self.track_names.get(t_idx).filter(|n| !n.is_empty()) {
                track_events.push(TrackEvent {
                    delta: u28::new(0),
                    kind: TrackEventKind::Meta(MetaMessage::TrackName(name.as_bytes())),
                });
            }

            // Tempo on Track 0
            if t_idx == 0 {
                track_events.push(TrackEvent {
                    delta: u28::new(0),
                    kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::new(
                        tempo_us.min(0x00FF_FFFF),
                    ))),
                });
            }

            // Gather notes for this track
            let mut sched: Vec<ScheduledEvent> = Vec::new();
            let mut programs_emitted: HashMap<u8, u8> = HashMap::new();

            for note in self.notes.iter().filter(|n| n.track == t_idx) {
                let start_tick = (note.start_sec as f64 * ticks_per_sec).round() as u64;
                let dur_ticks = ((note.duration_sec as f64 * ticks_per_sec).round() as u64).max(1);
                let end_tick = start_tick + dur_ticks;

                let ch = note.channel.min(15);
                if programs_emitted.get(&ch) != Some(&note.program) {
                    programs_emitted.insert(ch, note.program);
                    sched.push(ScheduledEvent {
                        tick: start_tick,
                        channel: ch,
                        priority: 0,
                        kind: NoteEventType::ProgramChange(note.program),
                    });
                }

                sched.push(ScheduledEvent {
                    tick: start_tick,
                    channel: ch,
                    priority: 2,
                    kind: NoteEventType::NoteOn {
                        key: note.key.min(127),
                        vel: note.velocity.clamp(1, 127),
                    },
                });

                sched.push(ScheduledEvent {
                    tick: end_tick,
                    channel: ch,
                    priority: 1,
                    kind: NoteEventType::NoteOff {
                        key: note.key.min(127),
                        vel: 0,
                    },
                });
            }

            sched.sort_by(|a, b| match a.tick.cmp(&b.tick) {
                std::cmp::Ordering::Equal => a.priority.cmp(&b.priority),
                other => other,
            });

            let mut last_tick = 0u64;
            for ev in sched {
                let delta = (ev.tick.saturating_sub(last_tick)).min(0x0FFF_FFFF) as u32;
                last_tick = ev.tick;

                let ch = u4::new(ev.channel);
                let mid_kind = match ev.kind {
                    NoteEventType::ProgramChange(prog) => TrackEventKind::Midi {
                        channel: ch,
                        message: MidiMessage::ProgramChange {
                            program: u7::new(prog.min(127)),
                        },
                    },
                    NoteEventType::NoteOn { key, vel } => TrackEventKind::Midi {
                        channel: ch,
                        message: MidiMessage::NoteOn {
                            key: u7::new(key),
                            vel: u7::new(vel),
                        },
                    },
                    NoteEventType::NoteOff { key, vel } => TrackEventKind::Midi {
                        channel: ch,
                        message: MidiMessage::NoteOff {
                            key: u7::new(key),
                            vel: u7::new(vel),
                        },
                    },
                };

                track_events.push(TrackEvent {
                    delta: u28::new(delta),
                    kind: mid_kind,
                });
            }

            // End of track marker
            track_events.push(TrackEvent {
                delta: u28::new(0),
                kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
            });

            smf.tracks.push(track_events);
        }

        let mut bytes = Vec::new();
        smf.write(&mut bytes)
            .map_err(|e| format!("Failed to serialize MIDI: {}", e))?;
        Ok(bytes)
    }

    /// Synthesizes the MIDI notes into an in-memory 44.1kHz 16-bit Mono WAV byte buffer,
    /// optionally filtering to a single soloed track.
    pub fn synthesize_tracks_to_wav(&self, solo_track: Option<usize>) -> Vec<u8> {
        let sample_rate: u32 = 44100;
        let total_samples = ((self.duration_sec + 0.6) * sample_rate as f32).ceil() as usize;
        let mut buffer = vec![0.0f32; total_samples];

        // LCG PRNG state for drum noise
        let mut rng_state: u32 = 123456789;

        for note in &self.notes {
            if let Some(st) = solo_track {
                if note.track != st {
                    continue;
                }
            }

            let start_sample = (note.start_sec * sample_rate as f32) as usize;
            if start_sample >= total_samples {
                continue;
            }

            let inst_type = self.resolve_instrument(note);
            let vel_scale = (note.velocity as f32 / 127.0).clamp(0.15, 1.0) * 0.28;

            match inst_type {
                InstrumentType::Drums => {
                    let drum_len = match note.key {
                        35 | 36 => 0.20,      // Kick
                        38 | 40 => 0.18,      // Snare
                        42 | 44 => 0.05,      // Closed Hat
                        46 => 0.28,           // Open Hat
                        49 | 51 | 57 => 0.65, // Crash / Ride
                        _ => 0.15,
                    };
                    let drum_samples = (drum_len * sample_rate as f32) as usize;
                    let end_sample = (start_sample + drum_samples).min(total_samples);

                    for s in start_sample..end_sample {
                        let t = (s - start_sample) as f32 / sample_rate as f32;
                        rng_state = rng_state.wrapping_mul(1664525).wrapping_add(1013904223);
                        let noise = (rng_state as f32 / u32::MAX as f32) * 2.0 - 1.0;

                        let wave = match note.key {
                            35 | 36 => {
                                // Kick: Pitch drop sweep + transient pop
                                let f = 42.0 + 90.0 * (-t * 32.0).exp();
                                let p = t * f * std::f32::consts::TAU;
                                let env = (-t * 16.0).exp();
                                let pop = if t < 0.004 { 0.4 } else { 0.0 };
                                p.sin() * env * 1.2 + pop
                            }
                            38 | 40 => {
                                // Snare: Body tone + filtered noise snap
                                let tone =
                                    (t * 180.0 * std::f32::consts::TAU).sin() * (-t * 22.0).exp();
                                let rattle = noise * (-t * 15.0).exp();
                                tone * 0.55 + rattle * 0.65
                            }
                            42 | 44 => {
                                // Closed Hi-Hat: Crisp high-frequency metallic sizzle
                                noise * (-t * 60.0).exp() * 0.5
                            }
                            46 => {
                                // Open Hi-Hat: Slower decaying noise
                                noise * (-t * 14.0).exp() * 0.55
                            }
                            49 | 51 | 57 => {
                                // Crash / Cymbal: Long shimmer
                                noise * (-t * 4.0).exp() * 0.4
                            }
                            _ => {
                                // Toms / Misc Percussion
                                let f = 80.0 + (note.key as f32) * 2.0;
                                let p = t * f * std::f32::consts::TAU;
                                p.sin() * (-t * 18.0).exp() * 0.8
                            }
                        };
                        buffer[s] += wave * vel_scale;
                    }
                }
                InstrumentType::Bass => {
                    let dur_samples = (note.duration_sec * sample_rate as f32) as usize;
                    let release_samples = (0.05 * sample_rate as f32) as usize;
                    let end_sample =
                        (start_sample + dur_samples + release_samples).min(total_samples);
                    let freq = note.frequency();

                    let attack_samples = (0.008 * sample_rate as f32) as usize;
                    let decay_samples = (0.06 * sample_rate as f32) as usize;

                    for s in start_sample..end_sample {
                        let note_t = (s - start_sample) as f32 / sample_rate as f32;
                        let rel = s - start_sample;

                        let env = if rel < attack_samples {
                            rel as f32 / attack_samples as f32
                        } else if rel < attack_samples + decay_samples {
                            1.0 - 0.25 * ((rel - attack_samples) as f32 / decay_samples as f32)
                        } else if s < start_sample + dur_samples {
                            0.75
                        } else {
                            let rel_progress =
                                (s - (start_sample + dur_samples)) as f32 / release_samples as f32;
                            0.75 * (1.0 - rel_progress).max(0.0)
                        };

                        let p = note_t * freq * std::f32::consts::TAU;
                        let wave = (p.sin() + 0.3 * (p * 2.0).sin() + 0.1 * (p * 3.0).sin())
                            .clamp(-1.0, 1.0);
                        buffer[s] += wave * env * vel_scale * 1.1;
                    }
                }
                InstrumentType::Piano => {
                    let dur_samples = (note.duration_sec * sample_rate as f32) as usize;
                    let release_samples = (0.15 * sample_rate as f32) as usize;
                    let end_sample =
                        (start_sample + dur_samples + release_samples).min(total_samples);
                    let freq = note.frequency();

                    let attack_samples = (0.004 * sample_rate as f32) as usize;

                    for s in start_sample..end_sample {
                        let note_t = (s - start_sample) as f32 / sample_rate as f32;
                        let rel = s - start_sample;

                        let env = if rel < attack_samples {
                            rel as f32 / attack_samples as f32
                        } else {
                            (-note_t * 3.2).exp()
                        };

                        let p = note_t * freq * std::f32::consts::TAU;
                        let wave = p.sin()
                            + 0.5 * (p * 2.0).sin() * (-note_t * 4.5).exp()
                            + 0.25 * (p * 3.0).sin() * (-note_t * 6.0).exp()
                            + 0.12 * (p * 4.0).sin() * (-note_t * 8.0).exp();
                        buffer[s] += wave * env * vel_scale * 0.9;
                    }
                }
                InstrumentType::StringsPad => {
                    let dur_samples = (note.duration_sec * sample_rate as f32) as usize;
                    let release_samples = (0.4 * sample_rate as f32) as usize;
                    let end_sample =
                        (start_sample + dur_samples + release_samples).min(total_samples);
                    let freq = note.frequency();

                    let attack_samples = (0.12 * sample_rate as f32) as usize;

                    for s in start_sample..end_sample {
                        let note_t = (s - start_sample) as f32 / sample_rate as f32;
                        let rel = s - start_sample;

                        let env = if rel < attack_samples {
                            rel as f32 / attack_samples as f32
                        } else if s < start_sample + dur_samples {
                            1.0
                        } else {
                            let rel_progress =
                                (s - (start_sample + dur_samples)) as f32 / release_samples as f32;
                            (1.0 - rel_progress).max(0.0)
                        };

                        let p1 = note_t * (freq * 0.998) * std::f32::consts::TAU;
                        let p2 = note_t * (freq * 1.002) * std::f32::consts::TAU;
                        let wave = 0.5 * (p1.sin() + p2.sin())
                            + 0.25 * ((p1 * 2.0).sin() + (p2 * 2.0).sin());
                        buffer[s] += wave * env * vel_scale * 0.7;
                    }
                }
                InstrumentType::Organ => {
                    let dur_samples = (note.duration_sec * sample_rate as f32) as usize;
                    let release_samples = (0.02 * sample_rate as f32) as usize;
                    let end_sample =
                        (start_sample + dur_samples + release_samples).min(total_samples);
                    let freq = note.frequency();

                    let attack_samples = (0.005 * sample_rate as f32) as usize;

                    for s in start_sample..end_sample {
                        let note_t = (s - start_sample) as f32 / sample_rate as f32;
                        let rel = s - start_sample;

                        let env = if rel < attack_samples {
                            rel as f32 / attack_samples as f32
                        } else if s < start_sample + dur_samples {
                            1.0
                        } else {
                            let rel_progress =
                                (s - (start_sample + dur_samples)) as f32 / release_samples as f32;
                            (1.0 - rel_progress).max(0.0)
                        };

                        let p = note_t * freq * std::f32::consts::TAU;
                        let wave = p.sin()
                            + 0.5 * (p * 2.0).sin()
                            + 0.3 * (p * 3.0).sin()
                            + 0.2 * (p * 4.0).sin();
                        buffer[s] += wave * env * vel_scale * 0.6;
                    }
                }
                InstrumentType::GuitarPluck => {
                    let dur_samples = (note.duration_sec * sample_rate as f32) as usize;
                    let release_samples = (0.1 * sample_rate as f32) as usize;
                    let end_sample =
                        (start_sample + dur_samples + release_samples).min(total_samples);
                    let freq = note.frequency();

                    let attack_samples = (0.002 * sample_rate as f32) as usize;

                    for s in start_sample..end_sample {
                        let note_t = (s - start_sample) as f32 / sample_rate as f32;
                        let rel = s - start_sample;

                        let env = if rel < attack_samples {
                            rel as f32 / attack_samples as f32
                        } else {
                            (-note_t * 5.5).exp()
                        };

                        let p = note_t * freq * std::f32::consts::TAU;
                        let wave = (p * 0.5).sin().signum() * 0.5 + p.sin() * 0.5;
                        buffer[s] += wave * env * vel_scale * 0.8;
                    }
                }
                InstrumentType::LeadBrass => {
                    let dur_samples = (note.duration_sec * sample_rate as f32) as usize;
                    let release_samples = (0.08 * sample_rate as f32) as usize;
                    let end_sample =
                        (start_sample + dur_samples + release_samples).min(total_samples);
                    let freq = note.frequency();

                    let attack_samples = (0.03 * sample_rate as f32) as usize;

                    for s in start_sample..end_sample {
                        let note_t = (s - start_sample) as f32 / sample_rate as f32;
                        let rel = s - start_sample;

                        let env = if rel < attack_samples {
                            rel as f32 / attack_samples as f32
                        } else if s < start_sample + dur_samples {
                            0.95
                        } else {
                            let rel_progress =
                                (s - (start_sample + dur_samples)) as f32 / release_samples as f32;
                            0.95 * (1.0 - rel_progress).max(0.0)
                        };

                        let p = note_t * freq * std::f32::consts::TAU;
                        let wave =
                            (2.0 * (p / std::f32::consts::TAU).fract() - 1.0) * 0.6 + p.sin() * 0.4;
                        buffer[s] += wave * env * vel_scale * 0.75;
                    }
                }
            }
        }

        // Soft-clip limiter to avoid harsh digital wrapping
        let mut max_val: f32 = 0.0;
        for &sample in &buffer {
            let abs = sample.abs();
            if abs > max_val {
                max_val = abs;
            }
        }

        let gain = if max_val > 0.95 { 0.95 / max_val } else { 1.0 };

        // Render 16-bit PCM RIFF WAV
        let num_channels: u16 = 1;
        let bits_per_sample: u16 = 16;
        let byte_rate = sample_rate * num_channels as u32 * (bits_per_sample / 8) as u32;
        let block_align = num_channels * (bits_per_sample / 8);
        let data_size = (buffer.len() * 2) as u32;

        let mut wav = Vec::with_capacity(44 + data_size as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_size).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&num_channels.to_le_bytes());
        wav.extend_from_slice(&sample_rate.to_le_bytes());
        wav.extend_from_slice(&byte_rate.to_le_bytes());
        wav.extend_from_slice(&block_align.to_le_bytes());
        wav.extend_from_slice(&bits_per_sample.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_size.to_le_bytes());

        for &sample in &buffer {
            let clamped = (sample * gain).clamp(-1.0, 1.0);
            let s16 = (clamped * 32767.0) as i16;
            wav.extend_from_slice(&s16.to_le_bytes());
        }

        wav
    }

    /// Synthesizes the MIDI notes into an in-memory 44.1kHz 16-bit Mono WAV byte buffer.
    /// Distinguishes drum channels and synthesizes rich instruments per track/program.
    pub fn synthesize_to_wav(&self) -> Vec<u8> {
        self.synthesize_tracks_to_wav(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_note_names() {
        let n = MidiNote {
            track: 0,
            channel: 0,
            program: 0,
            key: 60,
            velocity: 100,
            start_sec: 0.0,
            duration_sec: 1.0,
        };
        assert_eq!(n.note_name(), "C");
        assert_eq!(n.octave(), 4);
        assert_eq!(n.full_name(), "C4");
        assert!((n.frequency() - 261.63).abs() < 0.1);

        let a4 = MidiNote {
            track: 0,
            channel: 0,
            program: 0,
            key: 69,
            velocity: 100,
            start_sec: 0.0,
            duration_sec: 1.0,
        };
        assert_eq!(a4.full_name(), "A4");
        assert!((a4.frequency() - 440.0).abs() < 0.01);
    }

    #[test]
    fn test_wav_synthesis() {
        let midi = MidiData {
            title: "Test Multi-track".to_string(),
            track_count: 2,
            track_names: vec!["Piano".to_string(), "Drums".to_string()],
            total_notes: 4,
            duration_sec: 1.0,
            bpm: 120.0,
            min_key: 36,
            max_key: 67,
            notes: vec![
                MidiNote {
                    track: 0,
                    channel: 0,
                    program: 0,
                    key: 60,
                    velocity: 90,
                    start_sec: 0.0,
                    duration_sec: 0.8,
                },
                MidiNote {
                    track: 0,
                    channel: 0,
                    program: 0,
                    key: 64,
                    velocity: 85,
                    start_sec: 0.0,
                    duration_sec: 0.8,
                },
                MidiNote {
                    track: 0,
                    channel: 0,
                    program: 0,
                    key: 67,
                    velocity: 80,
                    start_sec: 0.0,
                    duration_sec: 0.8,
                },
                MidiNote {
                    track: 1,
                    channel: 9,
                    program: 0,
                    key: 36,
                    velocity: 100,
                    start_sec: 0.0,
                    duration_sec: 0.2,
                },
            ],
        };
        let wav = midi.synthesize_to_wav();
        assert!(wav.len() > 44);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
    }

    #[test]
    fn test_midi_slice_and_export_roundtrip() {
        let midi = MidiData {
            title: "Test Composition".to_string(),
            track_count: 2,
            track_names: vec!["Lead Synth".to_string(), "Bassline".to_string()],
            total_notes: 4,
            duration_sec: 4.0,
            bpm: 120.0,
            min_key: 36,
            max_key: 72,
            notes: vec![
                MidiNote {
                    track: 0,
                    channel: 0,
                    program: 80,
                    key: 60,
                    velocity: 90,
                    start_sec: 0.5,
                    duration_sec: 1.0,
                },
                MidiNote {
                    track: 0,
                    channel: 0,
                    program: 80,
                    key: 64,
                    velocity: 85,
                    start_sec: 1.8,
                    duration_sec: 1.0,
                },
                MidiNote {
                    track: 1,
                    channel: 1,
                    program: 38,
                    key: 36,
                    velocity: 100,
                    start_sec: 0.0,
                    duration_sec: 2.0,
                },
                MidiNote {
                    track: 1,
                    channel: 1,
                    program: 38,
                    key: 38,
                    velocity: 95,
                    start_sec: 2.2,
                    duration_sec: 1.5,
                },
            ],
        };

        // 1. Slice all tracks between 1.0s and 3.0s
        let slice_all = midi.slice(1.0, 3.0, None);
        assert_eq!(slice_all.track_count, 2);
        // All 4 notes intersect [1.0, 3.0]:
        // Note 0: start 0.5, dur 1.0 -> ends 1.5 > 1.0 (intersects)
        // Note 1: start 1.8, dur 1.0 -> intersects
        // Note 2: start 0.0, dur 2.0 -> ends 2.0 > 1.0 (intersects)
        // Note 3: start 2.2, dur 1.5 -> starts 2.2 < 3.0 (intersects)
        assert_eq!(slice_all.total_notes, 4);
        assert!((slice_all.duration_sec - 2.0).abs() < 0.01);

        // Export slice_all to MIDI bytes and re-parse
        let bytes = slice_all.to_midi_bytes().expect("to_midi_bytes failed");
        assert!(bytes.starts_with(b"MThd"));
        let parsed =
            MidiData::parse_bytes(&bytes, "Parsed".to_string()).expect("parse_bytes failed");
        assert_eq!(parsed.track_count, 2);
        assert_eq!(parsed.notes.len(), 4);

        // 2. Slice solo track 1 (Bassline) between 1.0s and 3.0s
        let slice_solo = midi.slice(1.0, 3.0, Some(1));
        assert_eq!(slice_solo.track_count, 1);
        assert_eq!(slice_solo.track_names, vec!["Bassline".to_string()]);
        assert_eq!(slice_solo.total_notes, 2); // only notes from track 1
        for n in &slice_solo.notes {
            assert_eq!(n.track, 0); // remapped to single track 0
            assert_eq!(n.channel, 1);
        }

        // Export solo slice to bytes and re-parse
        let solo_bytes = slice_solo
            .to_midi_bytes()
            .expect("solo to_midi_bytes failed");
        let parsed_solo = MidiData::parse_bytes(&solo_bytes, "Parsed Solo".to_string())
            .expect("parse solo failed");
        assert_eq!(parsed_solo.track_count, 1);
        assert_eq!(parsed_solo.track_names, vec!["Bassline".to_string()]);
        assert_eq!(parsed_solo.notes.len(), 2);

        // 3. Test solo audio synthesis
        let wav_solo0 = midi.synthesize_tracks_to_wav(Some(0));
        let wav_solo1 = midi.synthesize_tracks_to_wav(Some(1));
        let wav_full = midi.synthesize_tracks_to_wav(None);

        assert!(wav_solo0.len() > 44);
        assert!(wav_solo1.len() > 44);
        assert!(wav_full.len() > 44);
        assert_ne!(wav_solo0, wav_solo1);
    }
}
