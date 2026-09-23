//! `little-clip-core` provides fundamental domain models, clipboard item representations,
//! multi-slot history management, audio/video analysis, configuration definitions, and IPC contracts
//! for the Little Screen Clip ecosystem.

extern crate self as little_clip_core;

pub mod animation;
pub mod audio;
pub mod clipboard;
pub mod color;
pub mod config;
pub mod ipc;
pub mod item;
pub mod jobs;
/// Presentation-ready classifiers and decoded models shared by renderers and operations.
pub mod presentation {
    #[path = "../../../little-clip-ops/src/midi.rs"]
    pub mod midi;
    #[path = "../../../little-clip-ops/src/model3d.rs"]
    pub mod model3d;
    #[path = "../../../little-clip-ops/src/network_preview.rs"]
    pub mod network_preview;
    #[path = "../../../little-clip-ops/src/spreadsheet.rs"]
    pub mod spreadsheet;
    #[path = "../../../little-clip-ops/src/timestamps.rs"]
    pub mod timestamps;
    #[path = "../../../little-clip-ops/src/units.rs"]
    pub mod units;
}
pub mod pdf;
pub mod process;
pub mod raw_image;
pub mod rolling_audio;
pub mod slot;
pub mod staging;
pub mod syntax;
pub mod theme;
pub use color::parse_hex_color;
pub use theme::{AppPalette, DerivedHarmonies, Oklab, Oklch, Theme};

pub use animation::{
    AnimationThumbnail, decode_animated_bytes, decode_animated_gif_or_webp,
    extract_video_preview_frames, is_animated_image_path, is_video_path, read_video_metadata,
};
pub use audio::{
    AudioMetadata, AudioWaveform, CHROMATIC_NOTE_NAMES, ColorChordNoteInfo, FullAudioAnalysis,
    PeakBlock, compute_full_audio_analysis, compute_full_waveform, compute_plain_waveform,
    compute_spectral_colors, detect_pitch_yin, detect_transients, freq_to_note, is_audio_path,
    read_audio_metadata,
};
pub use clipboard::{
    ClipboardError, copy_bytes_to_system_clipboard, copy_file_uris_to_system_clipboard,
    copy_text_to_system_clipboard, file_uri_list,
};
pub use config::{AppConfig, AudioConfig, AudioSamplePosition, DisplayConfig, Model3dConfig};
pub use ipc::{IpcRequest, IpcResponse};
pub use item::{ClipItem, ClipPayload, ImageThumbnail, PayloadKind};
pub use jobs::{BackgroundJobs, JobError, JobTicket};
pub use presentation::{midi, model3d, network_preview, spreadsheet, timestamps, units};
pub use process::{CommandError, CommandOutput, CommandRequest, command_available};
pub use rolling_audio::{RollingAudioBuffer, slice_wav};
pub use slot::{HandSide, SlotId, SlotManager};
pub use staging::{ArtifactLease, ArtifactStore, StagingManager};
