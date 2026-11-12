//! Bounded streaming playback messages; decoded pixels never accumulate in a queue.
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

/// Largest accepted tightly packed RGBA frame (full HD).
pub const MAX_FRAME_BYTES: usize = 1920 * 1080 * 4;
const MAX_HEADER_BYTES: usize = 65536;

/// Playback rates exposed by the viewer.
pub const RATES: [f64; 8] = [0.1, 0.25, 0.5, 1.0, 1.5, 2.0, 4.0, 10.0];

/// Choices scoped to one viewing session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
    /// None selects the container's default audio track.
    pub audio_track: Option<u32>,
    /// Render embedded subtitles.
    pub subtitles_enabled: bool,
    /// None selects the first available subtitle track.
    pub subtitle_track: Option<u32>,
    /// Source-time multiplier.
    pub rate: f64,
    /// Restart this file at its end.
    pub looping: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            audio_track: None,
            subtitles_enabled: false,
            subtitle_track: None,
            rate: 1.0,
            looping: false,
        }
    }
}
impl Options {
    /// Rejects unsupported rates and out-of-budget stream ordinals.
    pub fn valid(&self) -> bool {
        RATES.contains(&self.rate)
            && self.audio_track.is_none_or(|n| n < 64)
            && self.subtitle_track.is_none_or(|n| n < 64)
    }
}

/// One bounded embedded-track description.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Track {
    /// Ordinal within this track type.
    pub index: u32,
    /// Human-readable title, language or codec.
    pub label: String,
}
/// Available tracks discovered during preroll.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Tracks {
    /// Audio tracks, excluding video/subtitle streams.
    pub audio: Vec<Track>,
    /// Embedded subtitle tracks.
    pub subtitles: Vec<Track>,
    /// Actual selected audio ordinal.
    pub current_audio: Option<u32>,
    /// Actual selected subtitle ordinal.
    pub current_subtitle: Option<u32>,
}
impl Tracks {
    fn valid(&self) -> bool {
        self.audio.len() <= 64
            && self.subtitles.len() <= 64
            && self
                .audio
                .iter()
                .chain(&self.subtitles)
                .all(|t| t.index < 64 && t.label.len() <= 512)
    }
}

/// Initial playback configuration for one inherited ordinary file.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// Ordinal among ordinary video streams, excluding attached pictures.
    pub stream: u32,
    /// Maximum output bounding box, preserving display aspect ratio.
    pub width: u32,
    /// Maximum output bounding box height.
    pub height: u32,
    /// Maximum delivered frames per second.
    pub fps: u32,
    /// Initial source-relative position.
    pub start_ms: u64,
    /// Probed source-relative end; zero means unknown.
    pub duration_ms: u64,
    /// Disable audio decoding and device acquisition for hover previews.
    pub silent: bool,
    /// Initial session choices; absent in older requests.
    #[serde(default)]
    pub options: Options,
}

/// A seek replaces earlier commands using its monotonically increasing epoch.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Control {
    /// Identifies frames belonging to this seek; unchanged for pause/volume.
    pub epoch: u64,
    /// Seek once to this source-relative position.
    pub seek_ms: Option<u64>,
    /// Desired playing state.
    pub playing: bool,
    /// Linear volume, bounded to zero through one.
    pub volume: f64,
    /// Desired session choices.
    #[serde(default)]
    pub options: Options,
}

/// A frame or state/error update, followed by exactly `bytes` raw RGBA bytes.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    /// Epoch accepted by the playback pipeline.
    pub epoch: u64,
    /// Observed source-relative playback position.
    pub position_ms: u64,
    /// Observed duration; zero means unknown.
    pub duration_ms: u64,
    /// Pipeline is playing.
    pub playing: bool,
    /// Source reached its end.
    pub ended: bool,
    /// Pixel width, or zero for a state-only update.
    pub width: u32,
    /// Pixel height, or zero for a state-only update.
    pub height: u32,
    /// Exactly width × height × four for a frame; zero for state only.
    pub bytes: usize,
    /// Terminal playback error, bounded by the header budget.
    pub error: Option<String>,
    /// Track catalogue, delivered independently from pixel frames.
    #[serde(default)]
    pub tracks: Option<Tracks>,
}

impl Update {
    fn validate(&self) -> io::Result<()> {
        if self.tracks.as_ref().is_some_and(|tracks| !tracks.valid()) {
            return Err(io::Error::other("Invalid playback track catalogue"));
        }
        let pixels = u64::from(self.width) * u64::from(self.height) * 4;
        if self.bytes > MAX_FRAME_BYTES
            || pixels != self.bytes as u64
            || self.width > 1920
            || self.height > 1920
            || (self.bytes == 0 && (self.width != 0 || self.height != 0))
        {
            return Err(io::Error::other("Invalid playback frame dimensions"));
        }
        Ok(())
    }

    /// Writes one bounded header and its tightly packed frame.
    pub fn write(&self, pixels: &[u8], output: &mut impl Write) -> io::Result<()> {
        self.validate()?;
        if pixels.len() != self.bytes {
            return Err(io::Error::other("Incomplete playback frame"));
        }
        let header = serde_json::to_vec(self)?;
        if header.len() > MAX_HEADER_BYTES {
            return Err(io::Error::other("Playback header exceeds budget"));
        }
        output.write_all(&(header.len() as u32).to_le_bytes())?;
        output.write_all(&header)?;
        output.write_all(pixels)?;
        output.flush()
    }

    /// Reads one bounded update, rejecting dimensions before allocating pixels.
    pub fn read(input: &mut impl Read) -> io::Result<(Self, Vec<u8>)> {
        let mut size = [0; 4];
        input.read_exact(&mut size)?;
        let size = u32::from_le_bytes(size) as usize;
        if size == 0 || size > MAX_HEADER_BYTES {
            return Err(io::Error::other("Invalid playback header length"));
        }
        let mut header = vec![0; size];
        input.read_exact(&mut header)?;
        let update: Self = serde_json::from_slice(&header)?;
        update.validate()?;
        let mut pixels = vec![0; update.bytes];
        input.read_exact(&mut pixels)?;
        Ok((update, pixels))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_frames_reject_invalid_headers_dimensions_and_truncated_pixels() {
        let frame = Update {
            epoch: 3,
            position_ms: 1250,
            duration_ms: 3000,
            playing: true,
            ended: false,
            width: 2,
            height: 1,
            bytes: 8,
            error: None,
            tracks: None,
        };
        let mut bytes = Vec::new();
        frame.write(&[12; 8], &mut bytes).unwrap();
        let (actual, pixels) = Update::read(&mut &bytes[..]).unwrap();
        assert_eq!(actual.epoch, 3);
        assert_eq!(pixels, [12; 8]);
        assert!(Update::read(&mut &bytes[..bytes.len() - 1]).is_err());
        assert!(Update::read(&mut &u32::MAX.to_le_bytes()[..]).is_err());
        for invalid in [
            Update {
                bytes: 7,
                ..frame.clone()
            },
            Update {
                width: 0,
                ..frame.clone()
            },
            Update {
                width: 1920,
                height: 1920,
                bytes: 1920 * 1920 * 4,
                ..frame
            },
        ] {
            let header = serde_json::to_vec(&invalid).unwrap();
            let mut bytes = (header.len() as u32).to_le_bytes().to_vec();
            bytes.extend_from_slice(&header);
            assert!(
                Update::read(&mut &bytes[..])
                    .unwrap_err()
                    .to_string()
                    .contains("dimensions")
            );
        }
    }
}
