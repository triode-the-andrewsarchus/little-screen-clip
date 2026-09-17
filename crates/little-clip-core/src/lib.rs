//! `little-clip-core` provides fundamental domain models, clipboard item representations,
//! multi-slot history management, audio/video analysis, configuration definitions, and IPC contracts
//! for the Little Screen Clip ecosystem.

pub mod animation;
pub mod audio;
pub mod color;
pub mod config;
pub mod ipc;
pub mod item;
pub mod jobs;
pub mod raw_image;
pub mod rolling_audio;
pub mod slot;
pub mod staging;
pub mod syntax;
pub use raw_image::*;
pub mod pdf;
pub use pdf::*;
pub mod theme;
pub use color::parse_hex_color;
pub use theme::{
    AppPalette, DerivedHarmonies, Oklab, Oklch, Theme, find_themes_dir, load_themes_from_dir,
};

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
pub use config::{
    AppConfig, AudioConfig, AudioSamplePosition, ColorConfig, DisplayConfig, Model3dConfig,
};
pub use ipc::{IpcRequest, IpcResponse};
pub use item::{ClipItem, ClipPayload, ImageThumbnail, PayloadKind};
pub use jobs::{BackgroundJobs, JobError, JobTicket};
pub use rolling_audio::{RollingAudioBuffer, slice_wav};
pub use slot::{HandSide, SlotId, SlotManager};
pub use staging::{ArtifactLease, ArtifactStore, StagingManager};
