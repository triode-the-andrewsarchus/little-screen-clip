pub mod animation;
pub mod audio;
pub mod config;
pub mod ipc;
pub mod item;
pub mod slot;
pub mod staging;
pub mod syntax;


pub use animation::{
    decode_animated_bytes, decode_animated_gif_or_webp, extract_video_preview_frames,
    is_animated_image_path, is_video_path, read_video_metadata, AnimationThumbnail,
};
pub use audio::{
    compute_full_waveform, compute_plain_waveform, compute_spectral_colors, is_audio_path,
    read_audio_metadata, AudioMetadata, AudioWaveform,
};
pub use config::{AppConfig, AudioConfig, AudioSamplePosition, ColorConfig, DisplayConfig};
pub use ipc::{IpcRequest, IpcResponse};
pub use item::{ClipItem, ClipPayload, ImageThumbnail, PayloadKind};
pub use slot::{HandSide, SlotId, SlotManager};
pub use staging::StagingManager;
