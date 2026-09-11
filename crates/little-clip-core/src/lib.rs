pub mod animation;
pub mod audio;
pub mod config;
pub mod ipc;
pub mod item;
pub mod rolling_audio;
pub mod slot;
pub mod staging;
pub mod syntax;
pub mod raw_image;
pub use raw_image::*;
pub mod pdf;
pub use pdf::*;


pub use animation::{
    decode_animated_bytes, decode_animated_gif_or_webp, extract_video_preview_frames,
    is_animated_image_path, is_video_path, read_video_metadata, AnimationThumbnail,
};
pub use audio::{
    compute_full_audio_analysis, compute_full_waveform, compute_plain_waveform,
    compute_spectral_colors, detect_pitch_yin, detect_transients, freq_to_note, is_audio_path,
    read_audio_metadata, AudioMetadata, AudioWaveform, ColorChordNoteInfo, FullAudioAnalysis,
    PeakBlock, CHROMATIC_NOTE_NAMES,
};
pub use config::{AppConfig, AudioConfig, AudioSamplePosition, ColorConfig, DisplayConfig};
pub use ipc::{IpcRequest, IpcResponse};
pub use item::{ClipItem, ClipPayload, ImageThumbnail, PayloadKind};
pub use rolling_audio::{slice_wav, RollingAudioBuffer};
pub use slot::{HandSide, SlotId, SlotManager};
pub use staging::StagingManager;
