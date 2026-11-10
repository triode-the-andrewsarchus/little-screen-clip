//! `little-clip-core` provides fundamental domain models, clipboard item representations,
//! multi-slot history management, audio/video analysis, configuration definitions, and IPC contracts
//! for the Little Screen Clip ecosystem.

pub mod animation;
pub mod audio;
pub mod calendar;
pub mod clipboard;
pub mod color;
pub mod config;
pub mod document;
pub mod ebook;
pub mod email;
pub mod file_format;
pub mod file_inventory;
pub mod file_source;
pub mod font_assets;
pub mod gallery_image;
pub mod html_preview;
pub mod image_document;
pub mod ipc;
pub mod item;
pub mod jobs;
pub mod kernel_interface;
pub mod kra_image;
pub mod text_format;
pub mod zip_content;
/// Presentation-ready classifiers and decoded models shared by renderers and operations.
pub mod presentation {
    pub mod midi;
    pub mod model3d;
    pub mod network_preview;
    pub mod spreadsheet;
    pub mod timestamps;
    pub mod units;
}
pub mod natural_sort;
pub mod palette;
pub mod path_address;
pub mod path_serde;
pub mod pcx_image;
pub mod pdf;
pub mod process;
pub mod psd_image;
pub mod raw_image;
pub mod rolling_audio;
pub mod slot;
pub mod staging;
pub mod subtitles;
pub mod svg_image;
pub mod syntax;
pub use color::parse_hex_color;
pub use palette::{AppPalette, BaseColors, DerivedHarmonies, Oklab, Oklch};

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

pub mod gcode;
pub mod geography;
pub mod multisample;
mod xml_content;

pub mod xcf_image;
