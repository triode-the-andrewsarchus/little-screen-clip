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
