//! Bounded streaming playback messages; decoded pixels never accumulate in a queue.
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

/// Largest accepted tightly packed RGBA frame (full HD).
pub const MAX_FRAME_BYTES: usize = 1920 * 1080 * 4;
const MAX_HEADER_BYTES: usize = 4096;

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
}

impl Update {
    fn validate(&self) -> io::Result<()> {
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
