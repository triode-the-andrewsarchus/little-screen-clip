//! Compact score previews share parsing, track colors, playback and MIDI export.
mod preview;
mod transport;
mod ui;
use little_clip_core::presentation::midi::MidiData;
pub(super) use preview::{load, render};
use std::{path::Path, sync::Arc};
pub(super) use transport::Preview;
pub(super) use ui::{focus_id, show};

pub(super) struct Document {
    pub data: Arc<MidiData>,
    pub low: u8,
    pub high: u8,
    pub cropped: usize,
    pub groups: Vec<(usize, u8)>,
}
pub(super) fn is_midi(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mid") || ext.eq_ignore_ascii_case("midi"))
}
impl Document {
    pub fn details(&self) -> String {
        let data = &self.data;
        let duration = data.duration_sec as u64;
        let key = |n: u8| {
            const NAMES: [&str; 12] = [
                "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
            ];
            format!("{}{}", NAMES[usize::from(n % 12)], i16::from(n) / 12 - 1)
        };
        let cropped = if self.cropped > 0 {
            format!(" · {} notes outside view", self.cropped)
        } else {
            String::new()
        };
        format!(
            "{} tracks · {} notes · {:.0} BPM · {}:{:02} · {}–{}{}",
            data.track_count,
            data.total_notes,
            data.bpm,
            duration / 60,
            duration % 60,
            key(self.low),
            key(self.high),
            cropped
        )
    }
}
