//! Standalone file-manager application and browser-owned services.
pub mod file_manager;
pub mod worker_cli;

pub(crate) use clip_studios::{
    audio_preview, desktop_ipc, file_source, image_preview, midi_view, model3d_view, onnx_runtime,
    pdf_view, shell, splat_view, spreadsheet_view, subtitle_view, text_preview, ui_ext,
};
pub(crate) use clip_ui::{hex_to_c32, icons, ui_appearance, ui_tabs};
