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
