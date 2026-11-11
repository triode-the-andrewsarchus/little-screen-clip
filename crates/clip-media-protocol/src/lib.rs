//! Data-only requests shared with the separately linked media worker.
//!
//! No codec, process or desktop dependencies belong in this crate.
use serde::{Deserialize, Serialize};

/// Maximum serialized render settings accepted by the worker.
pub const MAX_SETTINGS_BYTES: usize = 64 * 1024;

/// One admitted audio input; descriptor arguments travel separately.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioInput {
    /// Comma-separated allowed container names.
    pub formats: String,
    /// Explicit stream index, or the default audio stream.
    pub stream: Option<usize>,
}

/// Filter connections for a render request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AudioFilter {
    /// A single-input chain without external labels.
    Chain(String),
    /// A graph with input labels `0:a`, `1:a`, etc. and a named output.
    Graph {
        /// FFmpeg audio filtergraph description.
        spec: String,
        /// Output label without brackets.
        output: String,
    },
}

/// Audio export settings, independent of FFmpeg's CLI and codec bindings.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioRender {
    /// Protocol version, currently one.
    pub version: u32,
    /// Settings corresponding to the inherited input descriptors, in order.
    pub inputs: Vec<AudioInput>,
    /// Explicit output container (for example `wav` or `ipod`).
    pub format: String,
    /// Encoder name, or the container's default audio encoder.
    pub codec: Option<String>,
    /// Source-relative start/end seconds; only single-input jobs use a range.
    pub range: Option<(f64, f64)>,
    /// Processing chain/graph; absence means ordinary format conversion.
    pub filter: Option<AudioFilter>,
    /// Requested output sample rate; absence follows the processed audio and codec constraints.
    pub sample_rate: Option<u32>,
    /// Requested output channel count; absence follows the processed audio and codec constraints.
    pub channels: Option<u16>,
}

impl AudioRender {
    /// Creates a version-one request using the processed audio rate and layout.
    ///
    /// The container's default encoder is used until `codec` is set. Input
    /// count, encoder support and settings are validated by the worker before
    /// rendering; constructing settings does not authorize any file access.
    pub fn new(inputs: Vec<AudioInput>, format: impl Into<String>) -> Self {
        Self {
            version: 1,
            inputs,
            format: format.into(),
            codec: None,
            range: None,
            filter: None,
            sample_rate: None,
            channels: None,
        }
    }
}

/// Selection of a video stream in an admitted input.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum VideoStream {
    /// Global stream index, as returned in metadata.
    Index(usize),
    /// Ordinal among video streams, including attached artwork.
    Ordinal(usize),
    /// First ordinary video stream, excluding attached artwork.
    FirstNonCover,
}

/// One bounded PNG snapshot with its decoded source timestamp.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VideoFrame {
    /// Protocol version, currently one.
    pub version: u32,
    /// Allowed input containers.
    pub formats: String,
    /// Video stream selection.
    pub stream: VideoStream,
    /// Requested seconds relative to the container origin.
    pub seconds: f64,
    /// Select the first frame at/after the requested time; otherwise use the seek point.
    pub accurate: bool,
    /// Maximum output width.
    pub width: u32,
    /// Maximum output height.
    pub height: u32,
    /// Maximum decoded source pixel count, checked before and after decoding.
    pub max_source_pixels: u64,
}

impl VideoFrame {
    /// Creates a bounded ordinary-video snapshot request at the source start.
    pub fn new(formats: impl Into<String>, width: u32, height: u32) -> Self {
        Self {
            version: 1,
            formats: formats.into(),
            stream: VideoStream::FirstNonCover,
            seconds: 0.0,
            accurate: true,
            width,
            height,
            max_source_pixels: 8192 * 8192,
        }
    }
}

/// Header preceding one PNG response: little-endian u32 JSON size, JSON, PNG.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VideoFrameInfo {
    /// Actual decoded frame time relative to the container origin.
    pub seconds: f64,
    /// Oriented output width.
    pub width: u32,
    /// Oriented output height.
    pub height: u32,
    /// Exact following PNG byte count.
    pub encoded_bytes: u32,
}

/// One verified frame header and its borrowed encoded PNG payload.
pub struct EncodedVideoFrame<'a> {
    /// Actual decoded time and dimensions.
    pub info: VideoFrameInfo,
    /// Encoded PNG, borrowed without copying into another response buffer.
    pub png: &'a [u8],
}

/// Parses a complete still-frame response within the requested output bounds.
///
/// Empty output means no frame at the requested position. The caller must first
/// check child success and capture truncation; this function validates the wire
/// record, not the process outcome or the PNG's compressed pixel data.
///
/// # Errors
/// Returns an error for incomplete, oversized or inconsistent records.
pub fn parse_video_frame(
    bytes: &[u8],
    width: u32,
    height: u32,
) -> Result<Option<EncodedVideoFrame<'_>>, String> {
    if bytes.is_empty() {
        return Ok(None);
    }
    let size = bytes.get(..4).ok_or("Incomplete video frame prefix")?;
    let size =
        u32::from_le_bytes(size.try_into().map_err(|_| "Invalid video frame prefix")?) as usize;
    if size > 1024 {
        return Err("Video frame header exceeds its budget".into());
    }
    let header = bytes
        .get(4..4 + size)
        .ok_or("Incomplete video frame header")?;
    let info: VideoFrameInfo = serde_json::from_slice(header).map_err(|error| error.to_string())?;
    let png = &bytes[4 + size..];
    if info.width == 0
        || info.height == 0
        || info.width > width
        || info.height > height
        || !info.seconds.is_finite()
        || info.seconds.abs() >= 9.0e9
        || info.encoded_bytes as usize != png.len()
        || png.len() > 32 * 1024 * 1024
        || !png.starts_with(b"\x89PNG\r\n\x1a\n")
    {
        return Err("Invalid video frame payload or dimensions".into());
    }
    Ok(Some(EncodedVideoFrame { info, png }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frame_records_require_exact_payloads_dimensions_and_timestamps() {
        let info = VideoFrameInfo {
            seconds: 1.25,
            width: 80,
            height: 40,
            encoded_bytes: 8,
        };
        let header = serde_json::to_vec(&info).unwrap();
        let mut bytes = (header.len() as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        let frame = parse_video_frame(&bytes, 80, 40).unwrap().unwrap();
        assert_eq!(frame.info.seconds, 1.25);
        assert!(parse_video_frame(&bytes, 79, 40).is_err());
        assert!(parse_video_frame(&bytes[..bytes.len() - 1], 80, 40).is_err());
        bytes.push(0);
        assert!(parse_video_frame(&bytes, 80, 40).is_err());
        assert!(parse_video_frame(&[255; 4], 80, 40).is_err());
        let missing_time = br#"{"width":80,"height":40,"encoded_bytes":8}"#;
        let mut record = (missing_time.len() as u32).to_le_bytes().to_vec();
        record.extend_from_slice(missing_time);
        record.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        assert!(parse_video_frame(&record, 80, 40).is_err());
        assert!(parse_video_frame(&[], 80, 40).unwrap().is_none());
    }
}

/// A bounded fixed-rate MJPEG preview decoded in one worker session.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VideoPreview {
    /// Input selection, start time, orientation and output bounds.
    /// Sampling always follows the requested interval, regardless of `accurate`.
    pub frame: VideoFrame,
    /// Output frames per second (one through sixty).
    pub fps: u32,
    /// Maximum output frame count (one through 512).
    pub frames: u32,
    /// Optional source interval length, at most sixty seconds.
    pub duration: Option<f64>,
}
