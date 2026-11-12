//! Shared desktop studios, guarded file access and owned preview/export work.
//!
//! Hosts own window lifecycle, browser navigation and document selection. Sessions
//! retain their history, audition state and accepted operation completions.
pub use clip_ui::{hex_to_c32, hex_to_c32_alpha, icons, ui_appearance, ui_tabs};
use little_clip_core::ipc::{IpcRequest, IpcResponse};
pub use model3d_view::Model3dViewerState;
use std::time::Duration;
pub use ui_ext::*;

pub mod ansi;
pub mod audio_preview;
pub mod barcode_worker;
pub mod binary_diff_view;
pub mod conversion_decode_worker;
pub mod desktop_ipc;
pub mod diff_view;
pub use little_clip_core::file_source;
pub mod image_preview;
pub mod midi_view;
pub mod model3d_view;
pub mod onnx_runtime;
pub mod pdf_view;
pub mod shell;
pub mod splat_view;
pub mod spreadsheet_view;
pub mod subtitle_view;
pub mod text_preview;
pub mod ui_ext;
pub mod worker_cli;

pub fn send_ipc_request(req: &IpcRequest) -> Result<IpcResponse, Box<dyn std::error::Error>> {
    send_ipc_request_with_timeout(req, None)
}

pub fn send_ipc_request_with_timeout(
    req: &IpcRequest,
    timeout: Option<Duration>,
) -> Result<IpcResponse, Box<dyn std::error::Error>> {
    desktop_ipc::request(req, timeout, u64::MAX, usize::MAX)
}

pub mod diagram_preview;
