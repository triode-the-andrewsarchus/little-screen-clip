//! Shared desktop UI used by the workbench and the standalone file manager.

pub mod desktop_ipc;
pub mod file_manager;
pub mod icons;
pub mod ui_appearance;
pub mod ui_ext;
pub mod ui_tabs;

pub fn hex_to_c32(hex: &str) -> egui::Color32 {
    let (r, g, b, a) = little_clip_core::color::parse_hex_color_rgba_or_white(hex);
    egui::Color32::from_rgba_unmultiplied(r, g, b, a)
}

pub fn hex_to_c32_alpha(hex: &str, alpha: u8) -> egui::Color32 {
    let (r, g, b, _) = little_clip_core::color::parse_hex_color_rgba_or_white(hex);
    egui::Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

pub mod ansi;
pub mod audio_preview;
pub mod barcode_worker;
pub mod conversion_decode_worker;
pub mod image_preview;
pub mod midi_view;
pub mod model3d_view;
pub mod pdf_view;
pub mod shell;
pub mod splat_view;
pub mod spreadsheet_view;
pub mod text_preview;

pub use model3d_view::Model3dViewerState;
pub use ui_ext::*;

pub(crate) fn send_ipc_request(
    req: &little_clip_core::ipc::IpcRequest,
) -> Result<little_clip_core::ipc::IpcResponse, Box<dyn std::error::Error>> {
    send_ipc_request_with_timeout(req, None)
}
pub(crate) fn send_ipc_request_with_timeout(
    req: &little_clip_core::ipc::IpcRequest,
    timeout: Option<std::time::Duration>,
) -> Result<little_clip_core::ipc::IpcResponse, Box<dyn std::error::Error>> {
    desktop_ipc::request(req, timeout, u64::MAX, usize::MAX)
}

pub mod file_source;

pub mod worker_cli;
